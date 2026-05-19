use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct IrcConfig {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
    pub nickname: String,
    pub password: String,
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
            info!("IRC is disabled, skipping connection");
            std::future::pending::<()>().await;
        }

        let addr = format!("{}:{}", self.config.server, self.config.port);
        info!(server = %addr, nickname = %self.config.nickname, "Connecting to IRC");

        let stream = TcpStream::connect(&addr).await?;
        let (reader, mut writer) = stream.into_split();
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
                format!("USER {} 0 * :{}\r\n", self.config.nickname, self.config.nickname)
                    .as_bytes(),
            )
            .await?;
        writer.flush().await?;

        info!("IRC connection established");

        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                warn!("IRC connection closed");
                break;
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Handle PING to keep alive
            if line.starts_with("PING") {
                let pong = line.replace("PING", "PONG");
                writer.write_all(format!("{}\r\n", pong).as_bytes()).await?;
                writer.flush().await?;
                continue;
            }

            // Parse PRIVMSG
            if let Some(privmsg) = Self::parse_privmsg(line) {
                info!(
                    sender = %privmsg.sender,
                    message = %privmsg.message,
                    "Received IRC private message"
                );
                if self.message_tx.send(privmsg).await.is_err() {
                    error!("Failed to send IRC message to channel");
                }
            }
        }

        Ok(())
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
