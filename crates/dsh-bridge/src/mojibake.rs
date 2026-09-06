//! Detection and repair of Latin-1 double-encoded UTF-8 text ("mojibake").
//!
//! Observed failure mode (2026-08, Ollama cloud `glm-5.2` via the
//! `custom_ollama` provider route): some upstream streaming responses arrive
//! with each UTF-8 byte decoded as a separate Latin-1 character and
//! re-encoded as UTF-8, so e.g. `中` (E4 B8 AD) surfaces as the three
//! characters `ä` `¸` `­`. The corruption is lossless except where U+FFFD
//! already appeared, so it can be reversed: map every char back to its
//! Latin-1 byte, then decode the bytes as UTF-8.
//!
//! The detection signature is an adjacent pair `[U+00C2–U+00F4][U+0080–U+00BF]`
//! (a UTF-8 lead byte followed by a continuation byte, both seen as Latin-1
//! chars). Legitimate prose essentially never contains such pairs — the
//! U+0080–U+009F C1 controls are not text, and even accented European text
//! rarely places two high bytes adjacently. Repair is additionally guarded by
//! a full UTF-8 validation round-trip: a run of legitimate Latin-1 text (e.g.
//! `café`) fails to re-decode and is left untouched, so the worst case is
//! "not repaired", never "repaired into garbage".
//!
//! Two entry points:
//! - [`repair_mojibake`]: stateless repair of a complete string (finalized
//!   messages, history replay).
//! - [`StreamRepairer`]: stateful repair for streaming deltas, where a single
//!   original character may arrive split across deltas as individual Latin-1
//!   chars (`æ`, `\u{88}`, `\u{91}` = `我`). Holds back at most 3 trailing
//!   chars that form an incomplete sequence; once a stream has produced one
//!   validated repair it is "engaged" and single-sequence runs repair too.

/// Minimum adjacent lead+continuation pairs in a run before it is treated as
/// mojibake without further context. Two pairs already essentially never
/// occur in legitimate text; one pair (`Â°`, `Ã©`) is the classic mojibake of
/// a single 2-byte character but can appear in copy-pasted text, so the
/// stateless path requires two.
const MIN_SIGNATURE_HITS: usize = 2;

/// Hits required once a stream is known to be corrupted (a whole message is
/// one corruption domain, so after the first validated repair single
/// sequences repair too).
const ENGAGED_SIGNATURE_HITS: usize = 1;

/// Which assistant stream a [`StreamRepairer`] tracks, so a held-back tail
/// can be flushed to the matching [`acp_core::ClientEvent`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamTextKind {
    Text,
    Reasoning,
}

/// Count adjacent `[lead][continuation]` Latin-1 pairs — the mojibake
/// signature. UTF-8 lead bytes are C2–F4; continuation bytes are 80–BF.
pub fn signature_hits(text: &str) -> usize {
    let mut hits = 0;
    let mut prev_lead = false;
    for c in text.chars() {
        let cp = c as u32;
        if prev_lead && (0x80..=0xBF).contains(&cp) {
            hits += 1;
        }
        prev_lead = (0xC2..=0xF4).contains(&cp);
    }
    hits
}

/// Whether the text carries enough mojibake signature to be considered
/// corrupted without any streaming context. Part of the module API (e.g. for
/// surfacing a "suspected mojibake" badge); only exercised by tests today.
#[allow(dead_code)]
pub fn looks_like_mojibake(text: &str) -> bool {
    signature_hits(text) >= MIN_SIGNATURE_HITS
}

/// Repair Latin-1 double-encoded spans in a complete string. Runs of clean
/// text (including all chars above U+00FF, e.g. intact CJK) pass through
/// unchanged; only contiguous ≤U+00FF runs that both carry the signature and
/// survive the validation round-trip are decoded. Returns the input unchanged
/// when nothing qualifies.
pub fn repair_mojibake(text: &str) -> String {
    repair_with_threshold(text, MIN_SIGNATURE_HITS).0
}

/// Core repair: split into maximal runs of chars ≤ U+00FF (map-able back to
/// bytes) broken by chars > U+00FF (already-intact text, U+FFFD, fullwidth
/// punctuation). Each run with at least `min_hits` signature pairs is mapped
/// back to bytes and UTF-8 decoded; the decode is only accepted when it
/// - succeeds for the whole run (legit Latin-1 like `café` fails here: a lone
///   E9 byte is an incomplete sequence),
/// - introduces no C1 controls or U+FFFD, and
/// - strictly reduces the signature (guards against fixed points).
///
/// Returns the repaired string and whether any run was actually repaired.
fn repair_with_threshold(text: &str, min_hits: usize) -> (String, bool) {
    if signature_hits(text) < min_hits {
        return (text.to_string(), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let mut repaired_any = false;
    let mut flush = |run: &mut String, out: &mut String| {
        if run.is_empty() {
            return;
        }
        if signature_hits(run) >= min_hits
            && let Some(decoded) = try_decode_run(run)
        {
            tracing::debug!(
                target: "dsh-bridge::mojibake",
                run_chars = run.len(),
                hits = signature_hits(run),
                "repaired latin-1 double-encoded run"
            );
            out.push_str(&decoded);
            run.clear();
            repaired_any = true;
            return;
        }
        out.push_str(run);
        run.clear();
    };
    for c in text.chars() {
        if (c as u32) <= 0xFF {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    (out, repaired_any)
}

/// Map a ≤U+00FF run back to bytes and decode as UTF-8, with post-validation.
fn try_decode_run(run: &str) -> Option<String> {
    let bytes: Vec<u8> = run.chars().map(|c| c as u8).collect();
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded
        .chars()
        .any(|c| (0x80..=0x9F).contains(&(c as u32)) || c == '\u{FFFD}')
    {
        return None;
    }
    if signature_hits(&decoded) >= signature_hits(run) {
        return None;
    }
    Some(decoded)
}

/// Byte index of the lead char of the first signature pair, if any.
fn first_hit_index(text: &str) -> Option<usize> {
    let mut prev: Option<(usize, char)> = None;
    for (i, c) in text.char_indices() {
        if let Some((pi, pc)) = prev
            && (0xC2..=0xF4).contains(&(pc as u32))
            && (0x80..=0xBF).contains(&(c as u32))
        {
            return Some(pi);
        }
        prev = Some((i, c));
    }
    None
}

/// Stateful repair for streaming text deltas.
///
/// Multi-byte characters corrupted into Latin-1 arrive one byte at a time
/// (`æ`, `\u{88}`, `\u{91}`), so a delta can end mid-sequence. `push` first
/// holds back any trailing incomplete sequence (≤3 chars); the remaining
/// complete text is emitted once it carries enough signature to repair
/// confidently — before engagement that is [`MIN_SIGNATURE_HITS`] pairs, so
/// the first couple of corrupted characters accumulate until the second
/// sequence completes, then decode together. The first validated repair
/// "engages" the streamer: the whole stream is one corruption domain, so from
/// then on single sequences (one pair) repair immediately on completion.
///
/// Clean streams never engage and never buffer: the fast path passes deltas
/// through untouched, and legit Latin-1 runs fail the decode round-trip.
#[derive(Default)]
pub struct StreamRepairer {
    pending: String,
    engaged: bool,
}

impl StreamRepairer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one delta; returns the text safe to emit now (possibly empty while
    /// a corrupted sequence or the pre-engagement context is still
    /// accumulating).
    pub fn push(&mut self, delta: &str) -> String {
        // Fast path: no held-back state and no possible lead byte → clean.
        if self.pending.is_empty() && !delta.chars().any(|c| (0xC2..=0xF4).contains(&(c as u32))) {
            return delta.to_string();
        }
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(delta);
        let (complete, tail) = split_holdback(buf);
        let min_hits = if self.engaged {
            ENGAGED_SIGNATURE_HITS
        } else {
            MIN_SIGNATURE_HITS
        };
        if signature_hits(&complete) >= min_hits {
            let (repaired, repaired_any) = repair_with_threshold(&complete, min_hits);
            self.engaged |= repaired_any;
            self.pending = tail;
            return repaired;
        }
        // Not enough context to decide yet: emit the hit-free prefix now and
        // hold from the first signature pair so the evidence accumulates
        // across deltas instead of leaking half-judged mojibake.
        match first_hit_index(&complete) {
            Some(idx) => {
                self.pending = format!("{}{}", &complete[idx..], tail);
                complete[..idx].to_string()
            }
            None => {
                self.pending = tail;
                complete
            }
        }
    }

    /// Flush the held-back tail (block end / message finalize). Whatever
    /// still validates is repaired; the rest is returned raw.
    pub fn flush(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return pending;
        }
        let min_hits = if self.engaged {
            ENGAGED_SIGNATURE_HITS
        } else {
            MIN_SIGNATURE_HITS
        };
        repair_with_threshold(&pending, min_hits).0
    }
}

/// Split off a trailing incomplete corrupted sequence (≤3 chars) so `push`
/// never emits half a character. Only a trailing ≤U+00FF run whose byte
/// mapping ends in an *incomplete* UTF-8 sequence is held; runs that are
/// merely invalid (legit Latin-1 like a trailing `é` followed by more text)
/// are left in the emit half, where the repair pass keeps them raw.
fn split_holdback(buf: String) -> (String, String) {
    // Trailing run of chars ≤ U+00FF (each maps to exactly one byte).
    let mut run_start = buf.len();
    for (i, c) in buf.char_indices().rev() {
        if (c as u32) <= 0xFF {
            run_start = i;
        } else {
            break;
        }
    }
    if run_start == buf.len() {
        return (buf, String::new());
    }
    let run = &buf[run_start..];
    let bytes: Vec<u8> = run.chars().map(|c| c as u8).collect();
    match std::str::from_utf8(&bytes) {
        Ok(_) => (buf, String::new()),
        Err(e) if e.error_len().is_none() => {
            // Incomplete sequence at the end: hold back its chars (1–3).
            let hold_chars = bytes.len() - e.valid_up_to();
            let split_at = buf
                .char_indices()
                .rev()
                .nth(hold_chars - 1)
                .map(|(i, _)| i)
                .unwrap_or(buf.len());
            let hold = buf[split_at..].to_string();
            (buf[..split_at].to_string(), hold)
        }
        Err(_) => (buf, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the corruption exactly: every UTF-8 byte becomes one Latin-1
    /// char. The first test anchors this helper to a real captured sample.
    fn mangle(s: &str) -> String {
        s.bytes().map(|b| b as char).collect()
    }

    #[test]
    fn mangle_matches_captured_sample() {
        // Real sample from dsh session 843abfd8 (2026-08-24, glm-5.2 via
        // custom_ollama): the assistant's answer arrived as the mangled form.
        // (Expected string spelled with escapes: the mangled form contains C1
        // control chars that do not survive as source literals.)
        assert_eq!(
            mangle("启动时命令行窗口一闪而过，通常是"),
            "å\u{90}¯å\u{8a}¨æ\u{97}¶å\u{91}½ä»¤è¡\u{8c}çª\u{97}å\u{8f}£ä¸\u{80}é\u{97}ªè\u{80}\u{8c}è¿\u{87}ï¼\u{8c}é\u{80}\u{9a}å¸¸æ\u{98}¯"
        );
    }

    #[test]
    fn repairs_fully_corrupted_text() {
        let corrupted = mangle("启动时命令行窗口一闪而过，通常是有程序/脚本在后台短暂运行后退出。");
        assert!(looks_like_mojibake(&corrupted));
        assert_eq!(
            repair_mojibake(&corrupted),
            "启动时命令行窗口一闪而过，通常是有程序/脚本在后台短暂运行后退出。"
        );
    }

    #[test]
    fn repairs_only_the_corrupted_span_in_mixed_text() {
        // Real sample from dsh session 4645b746: clean prefix, corrupted tail.
        let mixed = format!(
            "正在进行修改。让我先从 `process.rs` {}",
            mangle("进行更改。")
        );
        assert_eq!(
            repair_mojibake(&mixed),
            "正在进行修改。让我先从 `process.rs` 进行更改。"
        );
    }

    #[test]
    fn keeps_legit_latin1_text_untouched() {
        // Accented European text fails the UTF-8 round-trip and must survive.
        for text in [
            "café au lait, naïve, élève, Zürich",
            "¿Cómo estás? ¡Bien! señor, anni, größe",
            "température: 25°C, fièvre",
        ] {
            assert_eq!(repair_mojibake(text), text, "should not touch: {text}");
        }
    }

    #[test]
    fn keeps_clean_cjk_and_ascii_untouched() {
        for text in [
            "启动时命令行窗口一闪而过",
            "hello world, cargo check -p dsh-bridge",
            "混合 mixed 文本 with emoji 🎉 and symbols ™ ©",
        ] {
            assert_eq!(repair_mojibake(text), text, "should not touch: {text}");
        }
    }

    #[test]
    fn fffd_breaks_runs_and_is_not_recovered() {
        // U+FFFD splits byte-runs; a fragment that is no longer a complete
        // UTF-8 sequence (e.g. the lead+continuation left when the third byte
        // was lost to FFFD) cannot validate and stays raw — the already-lost
        // information is unrecoverable, and we never guess.
        let mangled = mangle("都"); // E9 83 BD → three Latin-1 chars
        let fragment: String = mangled.chars().take(2).collect(); // E9 83: incomplete
        let with_fffd = format!("{fragment}\u{FFFD}");
        let repaired = repair_mojibake(&with_fffd);
        assert_eq!(repaired, with_fffd, "fragment must stay untouched");
        // Complete sequences split by a FFFD are each a single-pair run in the
        // stateless path — below MIN_SIGNATURE_HITS, so they conservatively
        // stay raw (the streaming path repairs them once engaged).
        let both_sides = format!("{}\u{FFFD}{}", mangle("测"), mangle("试"));
        assert_eq!(repair_mojibake(&both_sides), both_sides);
    }

    #[test]
    fn stream_repairs_single_byte_deltas() {
        // Real pattern from corrupted sessions: each byte arrives as its own
        // delta, so one Chinese character spans three deltas.
        let original = "我来试着用 pwsh 编译一下。";
        let mangled = mangle(original);
        let mut repairer = StreamRepairer::new();
        let mut out = String::new();
        for c in mangled.chars() {
            out.push_str(&repairer.push(&c.to_string()));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, original);
    }

    #[test]
    fn stream_holds_until_signature_threshold_then_decodes_together() {
        let mangled = mangle("我你");
        let chars: Vec<String> = mangled.chars().map(|c| c.to_string()).collect();
        let mut repairer = StreamRepairer::new();
        // First two bytes of 我 form an incomplete sequence: nothing emitted.
        assert_eq!(repairer.push(&chars[0]), "");
        assert_eq!(repairer.push(&chars[1]), "");
        // 我 completes but is a single signature pair — below the
        // pre-engagement threshold, so it keeps accumulating until 你
        // completes; then both characters decode together.
        let mut out = String::new();
        for c in &chars[2..] {
            out.push_str(&repairer.push(c));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, "我你");
    }

    #[test]
    fn stream_engages_and_repairs_later_single_sequences() {
        let mangled = mangle("编译通过，现在运行测试");
        let chars: Vec<String> = mangled.chars().map(|c| c.to_string()).collect();
        let mut repairer = StreamRepairer::new();
        let mut out = String::new();
        for c in &chars {
            out.push_str(&repairer.push(c));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, "编译通过，现在运行测试");
    }

    #[test]
    fn stream_passes_clean_text_through_untouched() {
        let mut repairer = StreamRepairer::new();
        let mut out = String::new();
        for delta in ["正在", "修改 process", ".rs：", "teardown 完成 🎉"] {
            out.push_str(&repairer.push(delta));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, "正在修改 process.rs：teardown 完成 🎉");
    }

    #[test]
    fn stream_keeps_legit_latin1_untouched() {
        let mut repairer = StreamRepairer::new();
        let mut out = String::new();
        for delta in ["café au", " lait, na", "ïve élève"] {
            out.push_str(&repairer.push(delta));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, "café au lait, naïve élève");
    }

    #[test]
    fn stream_handles_mojibake_mixed_with_clean_cjk() {
        // Corrupted span sandwiched between intact CJK (as seen in session
        // 4645b746): the clean chars break runs and pass through.
        let deltas = [
            "正在进行修改。",
            &mangle("让我先"),
            " clean ",
            &mangle("进行更改。"),
            " 完成。",
        ];
        let mut repairer = StreamRepairer::new();
        let mut out = String::new();
        for d in deltas {
            out.push_str(&repairer.push(d));
        }
        out.push_str(&repairer.flush());
        assert_eq!(out, "正在进行修改。让我先 clean 进行更改。 完成。");
    }

    #[test]
    fn signature_hit_count() {
        assert_eq!(signature_hits("hello world"), 0);
        assert_eq!(signature_hits("café"), 0); // é is a lone lead, no continuation follows
        assert_eq!(signature_hits(&mangle("我")), 1); // E6 88 91 → one pair
        assert_eq!(signature_hits(&mangle("测试")), 2);
        assert!(signature_hits(&mangle("启动时命令行窗口一闪而过")) >= 8);
    }
}
