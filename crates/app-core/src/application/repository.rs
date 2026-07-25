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
        GitService::stage_status_paths(&self.ui.workspace.root, paths).map_err(|e| e.to_string())?;
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
    pub fn generate_commit_message(
        &self,
        progress: &dyn Fn(&str),
    ) -> Result<String, String> {
        if self.is_remote_workspace() {
            return Err("远程工作区暂不支持 AI 生成提交信息".to_string());
        }

        let model = self.ui.session.model.clone();
        let config = SessionConfig {
            workspace_root: self.ui.workspace.root.display().to_string(),
            app_data_root: self.app_paths.root().display().to_string(),
            model,
            agent_command: self.agent_command.clone(),
            agent_env: crate::settings::agent_env_for_command(
                &self.agent_command,
                &self.app_paths,
            ),
            resume_session_id: None,
            log_id: make_log_id(),
            acp_port: self.acp_port,
            remote_ssh: None,
            mcp_servers: Vec::new(),
        };

        let prompt = format!(
            "在当前 Git 仓库里查看已暂存的变更，然后输出一条详细的 commit message。\n\
             建议先用只读命令建立全局视图，再按需深入细节，例如：\n\
             - `git status --short`\n\
             - `git diff --staged --stat`\n\
             - `git diff --staged --name-status`\n\
             - `git diff --staged`（内容过长或被截断时，再对关键文件用 `git diff --staged -- <path>`）\n\
             - 必要时用 `git log -8 --oneline` 参考近期提交风格\n\
             \n\
             输出格式必须是完整的多行 commit message：\n\
             1. 第一行：约定式提交标题（如 feat/fix/refactor/docs/test/chore: 描述），不超过 72 个字符，概括这次提交的核心意图\n\
             2. 空一行\n\
             3. 正文：用 2-6 条 `- ` 项目符号详细说明改动，覆盖：\n\
                - 改了什么模块/文件/能力\n\
                - 为什么改、解决了什么问题\n\
                - 关键行为变化、兼容性或风险点（如有）\n\
                - 测试/验证情况（如能从 diff 看出）\n\
             \n\
             要求：\n\
             - 尽量详细，优先写清动机与影响，不要只写空泛的 one-liner\n\
             - 只输出 commit message 本身，不要任何解释、前后缀、引号、代码块或 markdown 标题\n\
             - 不要输出思考过程、计划步骤或“Let me…”/“I now have…”这类旁白\n\
             - 不要包在 ``` 代码围栏里\n\
             - 只允许只读查看命令，不要 stage/unstage/commit/push，也不要修改任何文件。"
        );

        progress("正在启动 AI 会话…");
        let mut handle =
            SessionHandle::start(config).map_err(|e| format!("无法启动 AI 会话：{e}"))?;
        // Read-only permission: inspection commands such as `git diff` /
        // `git status` are auto-approved; mutating commands stay blocked.
        let _ = handle.set_permission_mode("plan");

        progress("正在查看已暂存的变更…");
        let task = handle.send_prompt_async(prompt);
        let collected = match task {
            Ok(mut task) => {
                // Only keep assistant text emitted after the latest tool call.
                // Earlier narration ("let me inspect…") is discarded so the
                // draft is not polluted with chain-of-thought preamble.
                let mut text = String::new();
                let mut run_error: Option<String> = None;
                while !task.is_finished() {
                    match task.wait_for_events(&mut handle) {
                        Ok(events) => {
                            for event in &events {
                                match event {
                                    ClientEvent::MessageChunk {
                                        role: workspace_model::MessageRole::Assistant,
                                        content,
                                    } => text.push_str(content),
                                    ClientEvent::ToolStarted { name, summary, .. } => {
                                        text.clear();
                                        let label = if summary.is_empty() {
                                            name.clone()
                                        } else {
                                            summary.clone()
                                        };
                                        progress(&format!("正在执行：{label}"));
                                    }
                                    ClientEvent::Interrupted { reason } => {
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
        "feat", "fix", "refactor", "docs", "test", "chore", "style", "perf",
        "build", "ci", "revert",
    ];
    let lower = line.to_ascii_lowercase();
    for ty in TYPES {
        if !lower.starts_with(ty) {
            continue;
        }
        let rest = &lower[ty.len()..];
        // type: / type!: / type(scope): / type(scope)!:
        if rest.starts_with(':') || rest.starts_with("!:") {
            let desc = rest
                .trim_start_matches('!')
                .trim_start_matches(':')
                .trim();
            return !desc.is_empty();
        }
        if let Some(after_scope) = rest.strip_prefix('(').and_then(|s| s.find(')').map(|i| &s[i + 1..]))
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
        let raw = "```\nCommit message:\nfix: repair dialog layout\n\n- switch input to textarea\n```\n";
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
        assert_eq!(message, "feat(ui): polish commit dialog\n\n- widen textarea");
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
