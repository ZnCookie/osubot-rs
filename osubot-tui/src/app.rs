use osubot_core::types::QQMessage;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Message {
    pub sender: String,
    pub text: String,
}

pub struct App {
    pub qq_id: i64,
    pub group_id: i64,
    pub messages: Vec<Message>,
    pub input: String,
    pub cursor_pos: usize,
    pub running: bool,
    pub output_dir: String,
    pub processing: bool,
    cmd_history: Vec<String>,
    history_idx: Option<usize>,
    tab_cycle: usize,
    tab_matches: Vec<String>,
}

impl App {
    pub fn new(qq_id: i64, group_id: i64) -> Self {
        let output_dir = std::env::current_dir()
            .map(|p| p.join("tui-output"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "tui-output".to_string());
        std::fs::create_dir_all(&output_dir).ok();
        let mut app = Self {
            qq_id,
            group_id,
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            running: true,
            output_dir,
            processing: false,
            cmd_history: Vec::new(),
            history_idx: None,
            tab_cycle: 0,
            tab_matches: Vec::new(),
        };
        app.push_system("osubot-tui — 交互式模拟器");
        app.push_system("");
        app.push_system("== 可用 Bot 命令 ==");
        app.push_system("  ~[模式]  where <用户名>  绑定/解绑  今日高光");
        app.push_system("  !profile [用户名]  !pr [用户名] [:模式]  !re [用户名] [:模式]");
        app.push_system("");
        app.push_system("== TUI 控制 ==");
        app.push_system("  /tui set-qq <id>  /tui set-group <id>  /tui clear  /tui exit");
        app.push_system("");
        app.push_system("== 与真实 osubot 的差异 ==");
        app.push_system("  · 未连接 OneBot/QQ，数据不回显到群聊");
        app.push_system("  · 无 Scheduler 后台定时更新，无 24h 变化对比");
        app.push_system("  · 绑定直接生效，跳过 IRC 验证码流程");
        app.push_system("  · @提及类命令 (查@、!profile + @) 不可用");
        app.push_system("  · 数据库与真实 bot 共享，操作会相互影响");
        app.push_system("");
        app.push_system("== 群成员模拟 ==");
        app.push_system("  · 从本地数据库获取所有已绑定用户模拟群成员");
        app.push_system("  · 无法区分真实群组");
        app
    }

    pub fn push_bot(&mut self, text: &str) {
        self.messages.push(Message {
            sender: "Bot".to_string(),
            text: text.to_string(),
        });
    }

    pub fn push_self(&mut self, text: &str) {
        self.messages.push(Message {
            sender: "你".to_string(),
            text: text.to_string(),
        });
    }

    pub fn push_system(&mut self, text: &str) {
        self.messages.push(Message {
            sender: "系统".to_string(),
            text: text.to_string(),
        });
    }

    pub fn handle_input(&mut self) -> Option<String> {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        let trimmed = input.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        self.push_self(&trimmed);
        Some(trimmed)
    }

    pub fn handle_tui_command(&mut self, cmd: &str) -> String {
        if let Some(args) = cmd.strip_prefix("/tui ") {
            let parts: Vec<&str> = args.splitn(2, ' ').collect();
            match parts[0] {
                "set-qq" => {
                    if let Some(id_str) = parts.get(1) {
                        if let Ok(id) = id_str.parse::<i64>() {
                            let old = self.qq_id;
                            self.qq_id = id;
                            return format!("QQ ID 已从 {} 切换为 {}", old, id);
                        }
                    }
                    return "用法: /tui set-qq <id>".to_string();
                }
                "set-group" => {
                    if let Some(id_str) = parts.get(1) {
                        if let Ok(id) = id_str.parse::<i64>() {
                            let old = self.group_id;
                            self.group_id = id;
                            return format!("群 ID 已从 {} 切换为 {}", old, id);
                        }
                    }
                    return "用法: /tui set-group <id>".to_string();
                }
                "clear" => {
                    self.messages.clear();
                    return String::new();
                }
                "exit" => {
                    self.running = false;
                    return "正在退出...".to_string();
                }
                _ => {
                    return format!(
                        "未知命令: /tui {}. 可用: set-qq, set-group, clear, exit",
                        parts[0]
                    );
                }
            }
        }
        "无效的命令".to_string()
    }

    pub fn is_tui_command(input: &str) -> bool {
        input.trim().starts_with("/tui ")
    }

    pub fn reset_completion(&mut self) {
        self.history_idx = None;
        self.tab_cycle = 0;
        self.tab_matches.clear();
    }

    pub fn push_history(&mut self, cmd: &str) {
        if self.cmd_history.last().map(|s| s.as_str()) != Some(cmd) {
            self.cmd_history.push(cmd.to_string());
        }
        self.history_idx = None;
    }

    pub fn history_up(&mut self) {
        if self.cmd_history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                self.history_idx = Some(self.cmd_history.len() - 1);
            }
            Some(0) => {}
            Some(i) => {
                self.history_idx = Some(i - 1);
            }
        }
        if let Some(idx) = self.history_idx {
            self.input = self.cmd_history[idx].clone();
            self.cursor_pos = self.input.len();
        }
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            Some(i) if i + 1 < self.cmd_history.len() => {
                self.history_idx = Some(i + 1);
                self.input = self.cmd_history[i + 1].clone();
            }
            Some(_) => {
                self.history_idx = None;
                self.input.clear();
            }
            None => {}
        }
        self.cursor_pos = self.input.len();
    }

    pub fn tab_complete(&mut self) {
        if let Some(rest) = self.input.strip_prefix("/tui ") {
            const TUI_COMMANDS: &[&str] = &["set-qq", "set-group", "clear", "exit"];
            if self.tab_cycle == 0 {
                self.tab_matches = TUI_COMMANDS
                    .iter()
                    .filter(|c| c.starts_with(rest))
                    .map(|s| s.to_string())
                    .collect();
            }
            if !self.tab_matches.is_empty() {
                let idx = self.tab_cycle % self.tab_matches.len();
                self.input = format!("/tui {}", self.tab_matches[idx]);
                self.cursor_pos = self.input.len();
                self.tab_cycle += 1;
            }
        }
    }

    pub fn save_image(&self, jpeg: &[u8], prefix: &str) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let filename = format!("{}/{}_{:x}.jpg", self.output_dir, prefix, timestamp);
        if let Err(e) = std::fs::write(&filename, jpeg) {
            format!("保存图片失败: {}", e)
        } else {
            format!("图片已保存: {}", filename)
        }
    }

    pub fn build_qq_message(&self, message: String) -> QQMessage {
        QQMessage {
            group_id: self.group_id,
            user_id: self.qq_id,
            message,
            mentioned_user_id: None,
        }
    }
}
