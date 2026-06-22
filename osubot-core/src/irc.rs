use crate::log_fmt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, trace, warn};

const READ_TIMEOUT_SECS: u64 = 120; // 2 minutes

#[derive(Clone)]
pub struct IrcConfig {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
    pub nickname: String,
    pub password: String,
}

impl std::fmt::Debug for IrcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrcConfig")
            .field("enabled", &self.enabled)
            .field("server", &self.server)
            .field("port", &self.port)
            .field("nickname", &self.nickname)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl IrcConfig {
    pub fn new(enabled: bool, server: &str, port: u16, nickname: &str, password: &str) -> Self {
        Self {
            enabled,
            server: server.to_string(),
            port,
            nickname: nickname.to_string(),
            password: password.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IrcPrivateMessage {
    pub sender: String,
    pub message: String,
}

pub struct IrcClient {
    config: IrcConfig,
    message_tx: mpsc::Sender<IrcPrivateMessage>,
}

impl IrcClient {
    pub fn new(config: IrcConfig, message_tx: mpsc::Sender<IrcPrivateMessage>) -> Self {
        Self { config, message_tx }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.config.enabled {
            info!("{}", log_fmt!("irc.disabled"));
            return Ok(());
        }

        let addr = format!("{}:{}", self.config.server, self.config.port);
        let mut reconnect_delay = tokio::time::Duration::from_secs(5);
        const MAX_RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(300);

        loop {
            info!(server = %addr, nickname = %self.config.nickname, "{}", log_fmt!("irc.connecting"));

            match self.connect_and_listen(&addr).await {
                Ok(()) => {
                    warn!("{}", log_fmt!("irc.listener_exited"));
                }
                Err(e) => {
                    warn!(error = %e, "{}", log_fmt!("irc.connection_error"));
                }
            }

            info!(
                delay_secs = reconnect_delay.as_secs(),
                "{}",
                log_fmt!("irc.waiting_reconnect")
            );
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
        }
    }

    async fn connect_and_listen(
        &self,
        addr: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // osu! IRC 服务端 (irc.ppy.sh:6667) 仅支持明文 TCP，不支持 TLS。
        // 这是 osu! 官方的限制，非本项目的设计选择。
        let tcp = TcpStream::connect(addr).await?;

        // Split stream into reader/writer halves
        let (reader, mut writer) = tcp.into_split();
        let mut reader = BufReader::new(reader);

        // Send PASS, NICK, USER
        writer
            .write_all(format!("PASS {}\r\n", self.config.password).as_bytes())
            .await?;
        writer
            .write_all(format!("NICK {}\r\n", self.config.nickname).as_bytes())
            .await?;
        writer
            .write_all(
                format!(
                    "USER {} 0 * :{}\r\n",
                    self.config.nickname, self.config.nickname
                )
                .as_bytes(),
            )
            .await?;
        writer.flush().await?;

        info!(nickname = %self.config.nickname, "{}", log_fmt!("irc.connected"));

        let mut line = String::new();
        let mut consecutive_timeouts: u32 = 0;
        loop {
            line.clear();
            match timeout(
                Duration::from_secs(READ_TIMEOUT_SECS),
                reader.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => {
                    warn!("{}", log_fmt!("irc.closed_by_server"));
                    return Ok(());
                }
                Ok(Ok(_)) => {
                    consecutive_timeouts = 0;
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "{}", log_fmt!("irc.read_error"));
                    return Err(e.into());
                }
                Err(_) => {
                    // Read timeout — connection may be dead
                    consecutive_timeouts += 1;
                    if consecutive_timeouts >= 2 {
                        warn!("{}", log_fmt!("irc.read_timeout_reconnect"));
                        return Ok(());
                    }
                    info!("{}", log_fmt!("irc.read_timeout_ping"));
                    if let Err(e) = writer.write_all(b"PING :keepalive\r\n").await {
                        warn!(error = %e, "{}", log_fmt!("irc.ping_failed"));
                        return Err(e.into());
                    }
                    writer.flush().await?;
                    continue;
                }
            }

            let line = line.trim();
            let line = strip_ircv3_tags(line);
            if line.is_empty() {
                continue;
            }

            // Handle PONG (response to our PING)
            if line.starts_with("PONG") {
                debug!(line = %line, "{}", log_fmt!("irc.pong_received"));
                continue;
            }

            // Handle PING to keep alive
            if line.starts_with("PING") {
                let pong = if let Some(rest) = line.strip_prefix("PING") {
                    format!("PONG{}", rest)
                } else {
                    line.to_string()
                };
                writer.write_all(format!("{}\r\n", pong).as_bytes()).await?;
                writer.flush().await?;
                continue;
            }

            // Log non-PRIVMSG messages for debugging
            if !line.contains("PRIVMSG") {
                trace!(line = %line, "{}", log_fmt!("irc.non_privmsg"));
            }

            // Parse PRIVMSG
            if let Some(privmsg) = Self::parse_privmsg(line) {
                info!(sender = %privmsg.sender, message = %privmsg.message, "{}", log_fmt!("irc.privmsg_received"));
                if self.message_tx.send(privmsg).await.is_err() {
                    error!("{}", log_fmt!("irc.send_failed"));
                }
            }
        }
    }

    fn parse_privmsg(line: &str) -> Option<IrcPrivateMessage> {
        // :username!~ident@host PRIVMSG target :message
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() < 4 {
            return None;
        }

        if parts[1] != "PRIVMSG" {
            return None;
        }

        let prefix = parts[0];
        let sender_nick = prefix.strip_prefix(':')?.split('!').next()?;

        let message = parts[3].strip_prefix(':')?.trim().to_string();

        Some(IrcPrivateMessage {
            sender: sender_nick.to_string(),
            message,
        })
    }
}

/// Strip IRCv3 message tags from a line. Returns the line without the leading
/// `@key=value;...` block, or the original line if no tags are present.
fn strip_ircv3_tags(line: &str) -> &str {
    line.strip_prefix('@')
        .and_then(|rest| rest.find(' ').map(|pos| &rest[pos + 1..]))
        .unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_privmsg_standard() {
        let result =
            IrcClient::parse_privmsg(":ZnCookie!~zncookie@user/zncookie PRIVMSG BotNick :123456");
        assert!(result.is_some());
        let msg = result.unwrap();
        assert_eq!(msg.sender, "ZnCookie");
        assert_eq!(msg.message, "123456");
    }

    #[test]
    fn parse_privmsg_after_tag_stripping() {
        // Simulate the tagged line arriving from osu! IRC (Bancho).
        // After tag stripping, parse_privmsg should parse it correctly.
        let raw = "@time=2026-05-22T13:21:53.547Z :ZnCookie!~zncookie@user/zncookie PRIVMSG BotNick :123456";
        let stripped = strip_ircv3_tags(raw);
        let result = IrcClient::parse_privmsg(stripped);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert_eq!(msg.sender, "ZnCookie");
        assert_eq!(msg.message, "123456");
    }

    #[test]
    fn strip_ircv3_tags_no_tags() {
        let line = ":server NOTICE * :Hello";
        assert_eq!(strip_ircv3_tags(line), line);
    }

    #[test]
    fn strip_ircv3_tags_with_tags() {
        let line = "@time=2026-05-22T13:21:53.547Z :nick!user@host PRIVMSG target :hello";
        assert_eq!(
            strip_ircv3_tags(line),
            ":nick!user@host PRIVMSG target :hello"
        );
    }

    #[test]
    fn parse_privmsg_no_match() {
        assert!(IrcClient::parse_privmsg(":server NOTICE * :Hello").is_none());
        assert!(IrcClient::parse_privmsg("PING :server").is_none());
    }
}
