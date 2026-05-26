mod app;

use app::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use osubot_core::{
    handler::{self, CommandResult, HandlerContext},
    storage::Storage,
    OauthTokenCache, RateLimiter,
};
use osubot_render::{
    render_profile_card, render_score_card, ScoreCardData, PROFILE_VIEWPORT_WIDTH,
};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::io::{self, Write};
use std::sync::{mpsc, Arc};

struct MpscWriter {
    tx: mpsc::Sender<String>,
}

impl Write for MpscWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf).to_string();
        let _ = self.tx.send(s);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let (log_tx, log_rx) = mpsc::channel::<String>();
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::sync::Mutex::new(MpscWriter { tx: log_tx }))
        .with_ansi(false)
        .with_target(false)
        .with_thread_ids(false)
        .with_line_number(false)
        .compact()
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let args: Vec<String> = std::env::args().collect();
    let mut qq_id: i64 = 0;
    let mut group_id: i64 = 0;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--qq" if i + 1 < args.len() => {
                qq_id = args[i + 1].parse().unwrap_or(0);
                i += 1;
            }
            "--group" if i + 1 < args.len() => {
                group_id = args[i + 1].parse().unwrap_or(0);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    let rt = tokio::runtime::Runtime::new()?;

    let config = osubot::config::Config::from_path("osubot.toml")?;
    let storage = Arc::new(Storage::new(&config.database.path).expect("Failed to open database"));
    let oauth = Arc::new(OauthTokenCache::new(
        config.osu.client_id,
        config.osu.api_key,
    ));
    let rate_limiter = Arc::new(rt.block_on(async { RateLimiter::new() }));

    std::fs::create_dir_all("cache").ok();
    std::fs::create_dir_all("tui-output").ok();

    let mut app = App::new(qq_id, group_id);

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = run_tui(
        &mut terminal,
        &mut app,
        &rt,
        storage,
        oauth,
        rate_limiter,
        log_rx,
    );

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_tui(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rt: &tokio::runtime::Runtime,
    storage: Arc<Storage>,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    log_rx: mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    loop {
        while let Ok(line) = log_rx.try_recv() {
            app.push_system(line.trim_end());
        }
        terminal.draw(|f| render(f, app))?;

        if !app.running {
            return Ok(());
        }

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Enter => {
                    if let Some(input) = app.handle_input() {
                        if App::is_tui_command(&input) {
                            let response = app.handle_tui_command(&input);
                            if !response.is_empty() {
                                app.push_system(&response);
                            }
                            app.push_history(&input);
                            app.reset_completion();
                            if !app.running {
                                return Ok(());
                            }
                        } else {
                            let all_bindings = match storage.get_all_user_bindings() {
                                Ok(b) => b,
                                Err(e) => {
                                    app.push_system(&format!(
                                        "数据库错误: 无法获取用户绑定列表 ({})",
                                        e
                                    ));
                                    app.push_history(&input);
                                    app.reset_completion();
                                    app.processing = false;
                                    continue;
                                }
                            };
                            let group_member_ids: Vec<i64> =
                                all_bindings.iter().map(|(qq, _, _)| *qq).collect();
                            let fetch_group_members: osubot_core::handler::GroupMemberFetcher = {
                                let ids = group_member_ids.clone();
                                Arc::new(move |_group_id| {
                                    let ids = ids.clone();
                                    Box::pin(async move { Ok(ids) })
                                })
                            };

                            let hctx = HandlerContext {
                                storage: storage.clone(),
                                oauth: oauth.clone(),
                                rate_limiter: rate_limiter.clone(),
                                trigger_update: None,
                                fetch_group_members: Some(fetch_group_members),
                            };

                            app.processing = true;
                            terminal.draw(|f| render(f, app))?;
                            let result = rt.block_on(handler::handle_command(
                                hctx,
                                app.build_qq_message(input.clone()),
                                None,
                            ));
                            app.processing = false;
                            app.push_history(&input);
                            app.reset_completion();
                            match result {
                                CommandResult::Text(text) => {
                                    app.push_bot(&text);
                                }
                                CommandResult::ProfileCard(data) => {
                                    let jpeg = rt.block_on(render_profile_card(
                                        &data.html,
                                        data.profile_hue,
                                        &data.avatar_url,
                                        &data.username,
                                        PROFILE_VIEWPORT_WIDTH,
                                        1200,
                                    ));
                                    match jpeg {
                                        Ok(bytes) => {
                                            let msg = app.save_image(&bytes, "profile");
                                            app.push_bot(&msg);
                                        }
                                        Err(e) => {
                                            app.push_bot(&format!("渲染个人主页失败: {}", e));
                                        }
                                    }
                                }
                                CommandResult::ScoreCard(sc) => {
                                    let jpeg =
                                        rt.block_on(render_score_card(ScoreCardData::from(&sc)));
                                    match jpeg {
                                        Ok(bytes) => {
                                            let msg = app.save_image(&bytes, "score");
                                            app.push_bot(&msg);
                                        }
                                        Err(e) => {
                                            app.push_bot(&format!("渲染成绩卡失败: {}", e));
                                        }
                                    }
                                }
                                CommandResult::None => {}
                            }
                        }
                    }
                }
                KeyCode::Char(c) => {
                    app.reset_completion();
                    app.input.insert(app.cursor_pos, c);
                    app.cursor_pos += 1;
                }
                KeyCode::Backspace if app.cursor_pos > 0 => {
                    app.cursor_pos -= 1;
                    app.input.remove(app.cursor_pos);
                }
                KeyCode::Left if app.cursor_pos > 0 => {
                    app.cursor_pos -= 1;
                }
                KeyCode::Right if app.cursor_pos < app.input.len() => {
                    app.cursor_pos += 1;
                }
                KeyCode::Esc => {
                    app.running = false;
                }
                KeyCode::Up => {
                    app.history_up();
                }
                KeyCode::Down => {
                    app.history_down();
                }
                KeyCode::Tab => {
                    app.tab_complete();
                }
                _ => {}
            }
        }
    }
}

fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    let status_text = format!("osubot-tui  QQ:{}  群:{}", app.qq_id, app.group_id);
    let status =
        Paragraph::new(status_text).style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(status, chunks[0]);

    let mut lines: Vec<ratatui::text::Line> = Vec::new();
    for msg in &app.messages {
        let sender_style = match msg.sender.as_str() {
            "Bot" => Style::default().fg(Color::Green),
            "你" => Style::default().fg(Color::Cyan),
            _ => Style::default().fg(Color::Yellow),
        };
        let prefix = format!("{} ", msg.sender);
        for (i, line) in msg.text.split('\n').enumerate() {
            if i == 0 {
                lines.push(ratatui::text::Line::from(vec![
                    Span::styled(prefix.clone(), sender_style),
                    Span::raw(line.to_string()),
                ]));
            } else {
                lines.push(ratatui::text::Line::from(Span::raw(line.to_string())));
            }
        }
    }

    let messages = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: true });
    f.render_widget(messages, chunks[1]);

    let input_text = if app.processing {
        format!("处理中... > {}", app.input)
    } else {
        format!("> {}", app.input)
    };
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title("Input"))
        .style(Style::default().fg(Color::White));
    f.render_widget(input, chunks[2]);
}
