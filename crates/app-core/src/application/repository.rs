use super::*;
use crate::remote_workspace::RemoteWorkspaceClient;

impl Application {
    pub fn replace_repository_snapshot(&mut self, snapshot: workspace_model::RepositorySnapshot) {
        if snapshot != self.ui.repository {
            self.ui.repository = snapshot;
            self.bump_revision();
        }
    }

    pub fn refresh_repository(&mut self) {
        if self.is_remote_workspace() {
            match self
                .remote_ssh
                .as_ref()
                .map(RemoteWorkspaceClient::new)
                .map(|client| client.git_status())
            {
                Some(Ok(snapshot)) if snapshot != self.ui.repository => {
                    self.ui.repository = snapshot;
                    self.bump_revision();
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None if !self.ui.repository.changed_files.is_empty() => {
                    self.ui.repository.changed_files.clear();
                    self.bump_revision();
                }
                Some(Err(_)) | None => {}
            }
            return;
        }

        match GitService::open(&self.ui.workspace.root) {
            Ok(snapshot) if snapshot != self.ui.repository => {
                self.ui.repository = snapshot;
                self.bump_revision();
            }
            Ok(_) => {}
            Err(_) if !self.ui.repository.changed_files.is_empty() => {
                self.ui.repository.changed_files.clear();
                self.bump_revision();
            }
            Err(_) => {}
        }
    }

    pub fn stage_files(&mut self, paths: &[String]) -> Result<(), String> {
        if self.is_remote_workspace() {
            // Only stage files that currently show up in the repository
            // status — a directory expands to the status-listed files under
            // it so ignored files are never swept in.
            let paths = expand_to_status_listed(&self.ui.repository, paths);
            if paths.is_empty() {
                return Ok(());
            }
            {
                let remote_ssh = self.remote_ssh.as_ref().ok_or_else(|| {
                    "Remote workspace is missing SSH session config for git stage".to_string()
                })?;
                RemoteWorkspaceClient::new(remote_ssh)
                    .git_stage(&paths)
                    .map_err(|error| format!("failed to stage remote files: {error}"))?;
            }
            self.refresh_repository();
            return Ok(());
        }

        self.ensure_local_workspace_for("local git commands")?;
        GitService::stage_status_paths(&self.ui.workspace.root, paths)
            .map_err(|e| e.to_string())?;
        self.refresh_repository();
        Ok(())
    }

    pub fn unstage_files(&mut self, paths: &[String]) -> Result<(), String> {
        if self.is_remote_workspace() {
            {
                let remote_ssh = self.remote_ssh.as_ref().ok_or_else(|| {
                    "Remote workspace is missing SSH session config for git unstage".to_string()
                })?;
                RemoteWorkspaceClient::new(remote_ssh)
                    .git_unstage(paths)
                    .map_err(|error| format!("failed to unstage remote files: {error}"))?;
            }
            self.refresh_repository();
            return Ok(());
        }

        self.ensure_local_workspace_for("local git commands")?;
        GitService::unstage(&self.ui.workspace.root, paths).map_err(|e| e.to_string())?;
        self.refresh_repository();
        Ok(())
    }

    pub fn commit_files(&mut self, message: &str) -> Result<(), String> {
        if self.is_remote_workspace() {
            {
                let remote_ssh = self.remote_ssh.as_ref().ok_or_else(|| {
                    "Remote workspace is missing SSH session config for git commit".to_string()
                })?;
                RemoteWorkspaceClient::new(remote_ssh)
                    .git_commit(message)
                    .map_err(|error| format!("failed to commit remote files: {error}"))?;
            }
            self.refresh_repository();
            return Ok(());
        }

        self.ensure_local_workspace_for("local git commands")?;
        GitService::commit(&self.ui.workspace.root, message).map_err(|e| e.to_string())?;
        self.refresh_repository();
        Ok(())
    }

    pub fn push_files(&mut self) -> Result<String, String> {
        if self.is_remote_workspace() {
            return Err("远程工作区暂不支持推送".to_string());
        }
        self.ensure_local_workspace_for("local git commands")?;
        let result = GitService::push(&self.ui.workspace.root).map_err(|e| e.to_string())?;
        self.refresh_repository();
        Ok(result)
    }

    /// Generate a commit-message draft by spinning up a throwaway sub-agent
    /// session with the current model settings. The agent is given read-only
    /// permission, so only inspection commands run — it inspects the staged
    /// changes itself and returns just the message. The temporary session is
    /// used once and shut down; it never touches the visible conversation.
    /// `progress` receives human-readable status updates as the agent works.
    /// Blocking — call off the UI thread.
    pub fn generate_commit_message(&self, progress: &dyn Fn(&str)) -> Result<String, String> {
        if self.is_remote_workspace() {
            return Err("远程工作区暂不支持 AI 生成提交信息".to_string());
        }

        let model = self.ui.session.model.clone();
        let config = SessionConfig {
            workspace_root: self.ui.workspace.root.display().to_string(),
            app_data_root: self.app_paths.root().display().to_string(),
            model,
            agent_command: self.agent_command.clone(),
            agent_env: crate::settings::agent_env_for_command(&self.agent_command, &self.app_paths),
            resume_session_id: None,
            log_id: make_log_id(),
            acp_port: self.acp_port,
            remote_ssh: None,
            mcp_servers: Vec::new(),
            harness_endpoint: None,
        };

        let prompt = format!(
            "你是 commit message 生成器。请直接在回复正文里给我一条 commit message，禁止调用任何写文件/编辑/创建文件的工具。\n\
             \n\
             第一步：用尽量少的只读命令了解已暂存变更。推荐一次性执行：\n\
             - `git diff --staged` 看完整 staged diff（若太长才改用 `--stat` + `--name-status`）\n\
             - 可选 `git log -5 --oneline` 参考提交风格\n\
             不要逐个文件反复跑 `git diff --staged -- <path>`，一次看全即可。\n\
             \n\
             第二步：看完后立即在回复正文输出 commit message，到此就结束，不要再做任何操作。\n\
             \n\
             格式：\n\
             1. 第一行：约定式提交标题（feat/fix/refactor/docs/test/chore: 描述），≤72 字符\n\
             2. 空一行\n\
             3. 正文：2-6 条 `- ` 项目符号，说明改了什么、为什么改、关键影响\n\
             \n\
             铁律（违反则任务失败）：\n\
             - 把 commit message 作为普通文本直接写在回复里，不要调用任何工具来创建、写入或保存文件（包括但不限于 write_file / edit_file / apply_patch / shell 重定向）\n\
             - 不要把结果保存到文件，不要创建 commit_message.txt 之类的文件\n\
             - 不要包在 ``` 代码围栏里，不要加引号、标签、markdown 标题\n\
             - 不要输出思考过程、“Let me…”/“Generated…”等旁白\n\
             - 只允许只读命令，禁止 stage/unstage/commit/push 或任何修改操作\n\
             - 全部内容就是这一条 commit message 本身。"
        );

        progress("正在启动 AI 会话…");
        let mut handle =
            SessionHandle::start(config).map_err(|e| format!("无法启动 AI 会话：{e}"))?;
        crate::startup_perf::mark("commit-gen/handle_started", "session handle created");
        // The throwaway session does NOT inherit the visible session's model —
        // `SessionConfig.model` only carries a display label, so without an
        // explicit model push the agent falls back to the default baked into
        // `config.toml`. That default frequently points at a model the BYOK
        // proxy can't serve, stalling the task with zero events until the
        // 120s timeout ("仍在等待 AI 响应…"). Push the current model first.
        if let Some((model_id, provider)) = self.current_model_for_background_session() {
            crate::startup_perf::mark(
                "commit-gen/set_model",
                format!("model={model_id:?} provider={provider:?}"),
            );
            // CodeBuddy/Codex agents expose the model as a config option
            // ("model") and do NOT implement `session/set_model` (returns
            // "Method not found"), so a `set_model` call leaves the session on
            // the `config.toml` default and the agent bails with an empty
            // refusal ("AI 没有返回可用的提交信息"). Prefer the config-option
            // path — the same one the main session uses when the user picks a
            // model — and fall back to `set_model` for agents that only
            // support the dedicated method.
            let config_option_result =
                handle.set_config_option("model", model_id.clone(), provider.clone());
            if let Err(config_option_error) = config_option_result {
                crate::startup_perf::mark(
                    "commit-gen/set_config_option_failed",
                    config_option_error.to_string(),
                );
                if let Err(model_error) = handle.set_model(model_id, provider) {
                    crate::startup_perf::mark(
                        "commit-gen/set_model_failed",
                        model_error.to_string(),
                    );
                }
            }
        } else {
            crate::startup_perf::mark("commit-gen/set_model_skipped", "no model resolved");
        }
        // Full access: the生成任务无权限 UI，plan/readonly 模式下任何触发
        // `Ask` 的命令都会让 broker 无限阻塞等用户回答（死锁，表现为
        // "仍在等待 AI 响应" 永不结束）。Full access 下 broker 直接放行，
        // 只读约束由 prompt（"只允许只读命令"）在 agent 层面保证。
        let _ = handle.set_permission_mode("full-access");

        progress("正在查看已暂存的变更…");
        let task = handle.send_prompt_async(prompt);
        crate::startup_perf::mark("commit-gen/prompt_dispatched", "prompt sent to worker");
        let collected = match task {
            Ok(mut task) => {
                // Only keep assistant text emitted after the latest tool call.
                // Earlier narration ("let me inspect…") is discarded so the
                // draft is not polluted with chain-of-thought preamble.
                let mut text = String::new();
                let mut run_error: Option<String> = None;
                // 非阻塞轮询 + 超时 + 心跳：ACP agent 可能长时间无响应
                // （网络/模型挂起/权限阻塞），同步 wait_for_events 会让整个
                // 任务假死，UI 只剩一句不再更新的进度。这里改为 try_recv
                // 轮询，定期发心跳进度，并在超时后主动放弃。
                const GENERATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
                const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
                const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
                let started_at = std::time::Instant::now();
                let mut last_heartbeat = started_at;
                let mut heartbeat_count = 0u32;
                while !task.is_finished() {
                    if started_at.elapsed() > GENERATE_TIMEOUT {
                        crate::startup_perf::mark(
                            "commit-gen/timeout",
                            format!(
                                "no response after {}s, collected_len={}",
                                GENERATE_TIMEOUT.as_secs(),
                                text.len()
                            ),
                        );
                        run_error = Some(format!(
                            "生成超时（{} 秒无响应），请检查 AI 服务后重试",
                            GENERATE_TIMEOUT.as_secs()
                        ));
                        break;
                    }
                    match task.collect_ready_events(&mut handle) {
                        Ok(events) => {
                            for event in &events {
                                match event {
                                    ClientEvent::MessageChunk {
                                        role: workspace_model::MessageRole::Assistant,
                                        content,
                                    } => {
                                        text.push_str(content);
                                        crate::startup_perf::mark(
                                            "commit-gen/chunk",
                                            format!("len={}", text.len()),
                                        );
                                    }
                                    ClientEvent::ToolStarted { name, summary, .. } => {
                                        text.clear();
                                        let label = if summary.is_empty() {
                                            name.clone()
                                        } else {
                                            summary.clone()
                                        };
                                        crate::startup_perf::mark("commit-gen/tool", label.clone());
                                        progress(&format!("正在执行：{label}"));
                                        last_heartbeat = std::time::Instant::now();
                                        // The prompt forbids write tools — catch a
                                        // misbehaving agent that saves the message
                                        // to a file instead of emitting it as text,
                                        // which would otherwise stall until timeout
                                        // with `collected_len=0`.
                                        if is_write_tool(name) {
                                            run_error = Some(format!(
                                                "AI 尝试写入文件（{name}）而非直接输出结果，已中止"
                                            ));
                                        }
                                    }
                                    ClientEvent::Interrupted { reason } => {
                                        crate::startup_perf::mark(
                                            "commit-gen/interrupted",
                                            reason.clone(),
                                        );
                                        run_error = Some(reason.clone());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            run_error = Some(e.to_string());
                            break;
                        }
                    }
                    // 心跳：长时间无事件时让 UI 知道任务仍存活
                    if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                        heartbeat_count += 1;
                        let dots = ".".repeat((heartbeat_count % 3 + 1) as usize);
                        progress(&format!(
                            "仍在等待 AI 响应{dots}（已等待 {} 秒）",
                            started_at.elapsed().as_secs()
                        ));
                        last_heartbeat = std::time::Instant::now();
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                for event in task.into_events() {
                    match &event {
                        ClientEvent::MessageChunk {
                            role: workspace_model::MessageRole::Assistant,
                            content,
                        } => text.push_str(content),
                        ClientEvent::ToolStarted { .. } => text.clear(),
                        _ => {}
                    }
                }
                match run_error {
                    Some(e) => Err(format!("AI 生成失败：{e}")),
                    None => Ok(text),
                }
            }
            Err(e) => Err(format!("AI 生成失败：{e}")),
        };
        handle.shutdown();

        let raw = collected?;
        progress("正在整理提交信息…");
        let message = sanitize_generated_commit_message(&raw);
        if message.is_empty() {
            return Err("AI 没有返回可用的提交信息".to_string());
        }
        Ok(message)
    }
}

/// Normalize an AI-produced commit message into a multi-line draft.
/// Keeps the subject + body, strips thinking preamble / code fences / labels /
/// surrounding quotes.
/// Detect tools that create/modify files. The commit-gen prompt forbids these
/// (the agent must emit the message as assistant text, not save it to disk);
/// returning true lets the loop bail out instead of stalling to timeout.
fn is_write_tool(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    const WRITE_TOOLS: &[&str] = &[
        "write_file",
        "writefile",
        "create_file",
        "createfile",
        "edit_file",
        "editfile",
        "apply_patch",
        "applypatch",
        "str_replace_editor",
        "rewrite_file",
        "write_to_file",
    ];
    WRITE_TOOLS
        .iter()
        .any(|tool| lower == *tool || lower.contains(tool))
}

fn sanitize_generated_commit_message(raw: &str) -> String {
    let mut text = raw.replace("\r\n", "\n").replace('\r', "\n");
    text = text.trim().to_string();

    // Prefer the last fenced block when the model wraps the message (or wraps
    // an intermediate draft). Ignore fences that don't look like a commit
    // message (e.g. code samples in the narration). Fall back to stripping a
    // single leading fence when the whole answer is wrapped.
    if let Some(extracted) = extract_last_fenced_commit_block(&text) {
        text = extracted;
    } else if text.starts_with("```") {
        let mut lines: Vec<&str> = text.lines().collect();
        if lines.first().is_some_and(|line| line.starts_with("```")) {
            lines.remove(0);
        }
        if lines.last().is_some_and(|line| line.trim() == "```") {
            lines.pop();
        }
        text = lines.join("\n").trim().to_string();
    }

    // Drop common leading labels the model sometimes emits (anywhere on a line).
    const LABELS: &[&str] = &[
        "commit message:",
        "commit message：",
        "提交信息:",
        "提交信息：",
        "message:",
        "message：",
    ];
    for label in LABELS {
        let lower = text.to_ascii_lowercase();
        let label_lower = label.to_ascii_lowercase();
        if lower.starts_with(&label_lower) {
            text = text[label.len()..].trim_start().to_string();
            break;
        }
    }

    // If narration precedes the real subject, cut everything before the last
    // conventional-commit subject line.
    if let Some(extracted) = extract_from_commit_subject(&text) {
        text = extracted;
    }

    // Preserve internal blank lines (subject/body separator) while trimming edges.
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut start = 0usize;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if start >= end {
        return String::new();
    }

    let mut cleaned = lines[start..end].join("\n");
    // Collapse 3+ blank lines down to a single blank line.
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }

    // Strip wrapping quotes around the whole message.
    let trimmed = cleaned.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('`') && trimmed.ends_with('`'))
    {
        return trimmed[1..trimmed.len() - 1].trim().to_string();
    }
    cleaned
}

/// Return the content of the last ``` ... ``` fenced block, if any.
fn extract_last_fenced_commit_block(text: &str) -> Option<String> {
    let mut last: Option<String> = None;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with("```") {
            continue;
        }
        let mut body: Vec<&str> = Vec::new();
        while let Some(inner) = lines.next() {
            if inner.trim() == "```" {
                break;
            }
            body.push(inner);
        }
        let content = body.join("\n").trim().to_string();
        if !content.is_empty() && content.lines().any(looks_like_commit_subject) {
            last = Some(content);
        }
    }
    last
}

/// Conventional-commit subject: `type(scope)!: description`
fn looks_like_commit_subject(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.len() > 100 {
        return false;
    }
    // Reject obvious prose / bullets / markdown.
    if line.starts_with('-')
        || line.starts_with('*')
        || line.starts_with('#')
        || line.starts_with('>')
        || line.starts_with('`')
    {
        return false;
    }
    const TYPES: &[&str] = &[
        "feat", "fix", "refactor", "docs", "test", "chore", "style", "perf", "build", "ci",
        "revert",
    ];
    let lower = line.to_ascii_lowercase();
    for ty in TYPES {
        if !lower.starts_with(ty) {
            continue;
        }
        let rest = &lower[ty.len()..];
        // type: / type!: / type(scope): / type(scope)!:
        if rest.starts_with(':') || rest.starts_with("!:") {
            let desc = rest.trim_start_matches('!').trim_start_matches(':').trim();
            return !desc.is_empty();
        }
        if let Some(after_scope) = rest
            .strip_prefix('(')
            .and_then(|s| s.find(')').map(|i| &s[i + 1..]))
        {
            if after_scope.starts_with(':') || after_scope.starts_with("!:") {
                let desc = after_scope
                    .trim_start_matches('!')
                    .trim_start_matches(':')
                    .trim();
                return !desc.is_empty();
            }
        }
    }
    false
}

/// If the text contains narration before a conventional-commit subject, return
/// the slice starting at the last such subject. When the first non-empty line
/// is already a subject, return None (nothing to strip).
fn extract_from_commit_subject(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut subject_indexes: Vec<usize> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if looks_like_commit_subject(line) {
            subject_indexes.push(idx);
        }
    }
    let subject_idx = *subject_indexes.last()?;

    // Nothing to strip if the message already starts at (or is only blank lines
    // before) the subject.
    let leading_nonempty = lines[..subject_idx]
        .iter()
        .any(|line| !line.trim().is_empty());
    if !leading_nonempty {
        return None;
    }

    Some(lines[subject_idx..].join("\n").trim().to_string())
}

#[cfg(test)]
mod generate_commit_message_tests {
    use super::sanitize_generated_commit_message;

    #[test]
    fn keeps_multiline_commit_message_body() {
        let raw = "feat: improve commit drafts\n\n- keep the subject concise\n- expand the body with details\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(
            message,
            "feat: improve commit drafts\n\n- keep the subject concise\n- expand the body with details"
        );
    }

    #[test]
    fn strips_code_fences_and_labels() {
        let raw =
            "```\nCommit message:\nfix: repair dialog layout\n\n- switch input to textarea\n```\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(
            message,
            "fix: repair dialog layout\n\n- switch input to textarea"
        );
    }

    #[test]
    fn collapses_excess_blank_lines() {
        let raw = "chore: tidy\n\n\n\n- one\n\n\n- two\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(message, "chore: tidy\n\n- one\n\n- two");
    }

    #[test]
    fn strips_thinking_preamble_before_subject() {
        let raw = "\
Now let me look at the remaining diff sections that were truncated:\
I now have a comprehensive view of the entire diff. Let me summarize the changes and produce the commit message.\n\
\n\
refactor: unified steel-blue accent across all themes with token-driven CSS\n\
\n\
- Replaces the previously split warm-teal accent pair with a single steel-blue family.\n\
- Migrates hardcoded color literals to semantic CSS custom properties.\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(
            message,
            "refactor: unified steel-blue accent across all themes with token-driven CSS\n\n\
- Replaces the previously split warm-teal accent pair with a single steel-blue family.\n\
- Migrates hardcoded color literals to semantic CSS custom properties."
        );
    }

    #[test]
    fn strips_preamble_and_keeps_scoped_subject() {
        let raw = "Looking at the staged diff now.\n\nfeat(ui): polish commit dialog\n\n- widen textarea\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(
            message,
            "feat(ui): polish commit dialog\n\n- widen textarea"
        );
    }

    #[test]
    fn prefers_last_fenced_commit_block() {
        let raw = "\
drafting…\n\
```\nnot the message\n```\n\
```\nfix: final draft\n\n- keep this one\n```\n";
        let message = sanitize_generated_commit_message(raw);
        assert_eq!(message, "fix: final draft\n\n- keep this one");
    }
}

/// Expand the requested paths to the files currently listed in the
/// repository status. A directory keeps only the status-listed files under
/// it; a file passes through only when it is itself listed. This keeps the
/// remote stage operation aligned with what the panel displays.
fn expand_to_status_listed(
    repository: &workspace_model::RepositorySnapshot,
    paths: &[String],
) -> Vec<String> {
    let mut expanded: Vec<String> = Vec::new();
    for raw in paths {
        let normalized = raw.replace('\\', "/").trim_end_matches('/').to_string();
        if normalized.is_empty() {
            continue;
        }
        let is_listed = repository
            .changed_files
            .iter()
            .any(|file| file.path.to_string_lossy().replace('\\', "/") == normalized);
        if is_listed {
            expanded.push(normalized.clone());
            continue;
        }
        let prefix = format!("{normalized}/");
        for file in &repository.changed_files {
            let file_path = file.path.to_string_lossy().replace('\\', "/");
            if file_path.starts_with(&prefix) && !expanded.contains(&file_path) {
                expanded.push(file_path);
            }
        }
    }
    expanded
}
