"""Repair historically inflated DeepSeek Harness usage rows in sessions.db.

Two defects in the (now fixed) dsh-bridge corrupted every `DeepSeek Harness`
row in `usage_events`:

1. **Duplicate rows** — dsh surfaces one model call's usage twice (a terminal
   `assistant/chunk` `{type:"usage"}` stream chunk plus the finalized
   `assistant/message` `usage` rollup) and history replay re-delivered both on
   every resume/reconnect, so each call's values were persisted 2x/4x/.../18x.
2. **Uncached-only input** — dsh's `TokenUsage` buckets are DISJOINT
   (`inputTokens` = uncached input; billed input = input + cacheRead +
   cacheWrite) but the rows stored `input_tokens` verbatim, so the cache axes
   were never folded into the cache-inclusive `input_tokens` that the rest of
   Kodex treats as an invariant.

This script repairs existing rows:

- Dedup exact-value duplicate groups per (session, input, output, cache_read,
  cache_write, reasoning, total), keeping the earliest row.
- Fold cache_read + cache_write into input_tokens on every remaining
  `DeepSeek Harness` row (turn_delta and session_total alike).

Run with the Maju desktop app CLOSED. Dry-run by default; pass --apply to
write. A backup copy of the database is created next to it before writing.

The cache fold is guarded by a `repair_meta` marker row so a re-run can never
fold already-folded rows a second time (the dedup step is idempotent).
"""

import argparse
import shutil
import sqlite3
import sys
from pathlib import Path

DB = Path.home() / ".kodex" / "sessions" / "sessions.db"
AGENT = "DeepSeek Harness"
FOLD_MARKER = "dsh_usage_cache_fold_v1"

DUPLICATE_GROUP = """
    SELECT session_id, input_tokens, output_tokens, cache_read_tokens,
           cache_write_tokens, reasoning_tokens, total_tokens,
           COUNT(*) AS copies, MIN(rowid) AS keep_rowid
    FROM usage_events
    WHERE agent_cli = ?
      AND scope = 'turn_delta'
    GROUP BY session_id, input_tokens, output_tokens, cache_read_tokens,
             cache_write_tokens, reasoning_tokens, total_tokens
    HAVING COUNT(*) > 1
"""


def summarize(cur):
    cur.execute(
        "SELECT COUNT(*), SUM(input_tokens), SUM(output_tokens),"
        " SUM(cache_read_tokens) FROM usage_events WHERE agent_cli = ?",
        (AGENT,),
    )
    n, s_in, s_out, s_cr = cur.fetchone()
    return n, s_in or 0, s_out or 0, s_cr or 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--apply", action="store_true", help="write the repair (default: dry run)"
    )
    args = parser.parse_args()

    if not DB.exists():
        print(f"database not found: {DB}")
        return 1

    uri = f"file:{DB.as_posix()}" + ("?mode=rw" if args.apply else "?mode=ro")
    con = sqlite3.connect(uri, uri=True)
    cur = con.cursor()

    rows, s_in, s_out, s_cr = summarize(cur)
    print(f"before: rows={rows} sum_in={s_in} sum_out={s_out} sum_cr={s_cr}")

    # 1. Duplicates ----------------------------------------------------------------
    cur.execute(DUPLICATE_GROUP, (AGENT,))
    groups = cur.fetchall()
    dup_rows = sum(g[7] - 1 for g in groups)
    print(f"duplicate groups: {len(groups)} (rows to delete: {dup_rows})")
    for g in groups[:5]:
        print(f"  e.g. session={g[0][:8]} in={g[1]} out={g[2]} cr={g[3]} copies={g[7]}")

    # 2. Fold cache axes into input ------------------------------------------------
    cur.execute(
        "SELECT COUNT(*) FROM usage_events WHERE agent_cli = ?"
        " AND input_tokens IS NOT NULL"
        " AND (COALESCE(cache_read_tokens, 0) + COALESCE(cache_write_tokens, 0)) > 0",
        (AGENT,),
    )
    foldable = cur.fetchone()[0]
    print(f"rows needing cache fold into input: {foldable}")

    cur.execute(
        "CREATE TABLE IF NOT EXISTS repair_meta (key TEXT PRIMARY KEY, value TEXT)"
    )
    cur.execute("SELECT value FROM repair_meta WHERE key = ?", (FOLD_MARKER,))
    marker = cur.fetchone()
    fold_done = marker is not None
    print(f"cache fold already applied (marker): {fold_done}")

    if not args.apply:
        print("\ndry run — rerun with --apply to write the repair")
        con.close()
        return 0

    backup = DB.with_suffix(".db.pre-usage-repair.bak")
    if not backup.exists():
        shutil.copy2(DB, backup)
        print(f"backup written: {backup}")

    cur.execute(
        """
        DELETE FROM usage_events
        WHERE agent_cli = ?
          AND scope = 'turn_delta'
          AND rowid NOT IN (
              SELECT MIN(rowid) FROM usage_events
              WHERE agent_cli = ? AND scope = 'turn_delta'
              GROUP BY session_id, input_tokens, output_tokens,
                       cache_read_tokens, cache_write_tokens, reasoning_tokens,
                       total_tokens
          )
        """,
        (AGENT, AGENT),
    )
    deleted = cur.rowcount
    print(f"deleted duplicate rows: {deleted}")

    if fold_done:
        print("cache fold: skipped (marker present — rows already folded once)")
    else:
        cur.execute(
            """
            UPDATE usage_events
            SET input_tokens = input_tokens
                + COALESCE(cache_read_tokens, 0)
                + COALESCE(cache_write_tokens, 0)
            WHERE agent_cli = ? AND input_tokens IS NOT NULL
            """,
            (AGENT,),
        )
        print(f"cache-folded rows: {cur.rowcount}")
        cur.execute(
            "INSERT OR REPLACE INTO repair_meta (key, value) VALUES (?, ?)",
            (FOLD_MARKER, "applied"),
        )

    con.commit()

    rows, s_in, s_out, s_cr = summarize(cur)
    print(f"after:  rows={rows} sum_in={s_in} sum_out={s_out} sum_cr={s_cr}")

    # Cross-check per session against the (authoritative, last-wins)
    # session_total rows.
    print("\nper-session check (turn_delta sums vs last session_total):")
    cur.execute(
        "SELECT session_id FROM usage_events WHERE agent_cli = ?"
        " GROUP BY session_id",
        (AGENT,),
    )
    sessions = [r[0] for r in cur.fetchall()]
    for sid in sessions:
        cur.execute(
            "SELECT SUM(input_tokens), SUM(output_tokens), COUNT(*)"
            " FROM usage_events WHERE session_id = ? AND scope = 'turn_delta'",
            (sid,),
        )
        t_in, t_out, n = cur.fetchone()
        cur.execute(
            "SELECT input_tokens, output_tokens FROM usage_events"
            " WHERE session_id = ? AND scope = 'session_total'"
            " ORDER BY created_at DESC, rowid DESC LIMIT 1",
            (sid,),
        )
        st = cur.fetchone()
        if st is None or t_in is None:
            continue
        print(
            f"  {sid[:8]}  calls={n:5d}  delta_in={t_in:>12,}  "
            f"total_in={st[0] or 0:>12,}  delta_out={t_out or 0:>10,}  "
            f"total_out={st[1] or 0:>10,}"
        )
    con.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
