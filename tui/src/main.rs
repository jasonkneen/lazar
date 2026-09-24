mod access;
mod agents_state;
mod app;
mod composer;
mod jobs;
mod kernel;
mod keys;
mod overlays;
mod providers;
mod render;
mod skills;
mod state;
mod status;
mod timeline;
mod transcript;

use app::{App, LaunchOptions};
use clap::{Parser, ValueEnum};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::{execute, terminal};
use ratatui::Terminal;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Icons {
    Nerd,
    Unicode,
    Ascii,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum FontWeight {
    Regular,
    Thin,
}

impl Icons {
    fn as_str(self) -> &'static str {
        match self {
            Self::Nerd => "nerd",
            Self::Unicode => "unicode",
            Self::Ascii => "ascii",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "lazartui",
    version,
    about = "Native terminal UI for the Lazar kernel"
)]
struct Args {
    /// Working directory used by Lazar's bash tool.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,
    /// Add a sandbox write root for this launch. Repeatable.
    #[arg(long = "add-dir", value_name = "PATH")]
    add_dirs: Vec<PathBuf>,
    /// Provider id from $LAZAR_HOME/providers.toml.
    #[arg(long)]
    provider: Option<String>,
    /// Model id override.
    #[arg(long)]
    model: Option<String>,
    /// Resume this Lazar session.
    #[arg(long)]
    session: Option<String>,
    /// Turn the tool sandbox on. Off by default.
    #[arg(long)]
    sandbox: bool,
    /// Force the tool sandbox off. This is the default.
    #[arg(long, conflicts_with = "sandbox")]
    no_sandbox: bool,
    /// Override the Lazar kernel executable.
    #[arg(long, value_name = "PATH")]
    kernel: Option<PathBuf>,
    /// Use ASCII fallbacks for decorative glyphs.
    #[arg(long, conflicts_with_all = ["icons", "nerd_font"])]
    ascii: bool,
    /// Icon mode: nerd, unicode, or ascii.
    #[arg(long, value_enum, conflicts_with_all = ["ascii", "nerd_font"])]
    icons: Option<Icons>,
    /// Shortcut for --icons nerd (requires a Nerd Font in the terminal).
    #[arg(long, conflicts_with_all = ["ascii", "icons"])]
    nerd_font: bool,
    /// Text emphasis: regular or thin. Thin avoids requesting bold faces.
    #[arg(long, value_enum, conflicts_with = "thin")]
    font_weight: Option<FontWeight>,
    /// Shortcut for --font-weight thin.
    #[arg(long, conflicts_with = "font_weight")]
    thin: bool,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            event::EnableBracketedPaste,
            event::EnableMouseCapture
        )?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            event::DisableMouseCapture,
            terminal::LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("lazartui requires an interactive terminal".into());
    }
    let icon_mode = if args.ascii {
        Some(Icons::Ascii)
    } else if args.nerd_font {
        Some(Icons::Nerd)
    } else {
        args.icons
    };
    if let Some(mode) = icon_mode {
        std::env::set_var("LAZARTUI_ICONS", mode.as_str());
    }
    let lazar_home = std::env::var_os("LAZAR_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is unset")).join("lazar")
        });
    let cwd = args.cwd.unwrap_or(std::env::current_dir()?);
    let mut app = App::new(LaunchOptions {
        lazar_home,
        cwd,
        add_dirs: args.add_dirs,
        provider: args.provider,
        model: args.model,
        session_id: args.session,
        no_sandbox: !args.sandbox || args.no_sandbox,
        kernel: args.kernel,
        thin: args
            .font_weight
            .map(|weight| matches!(weight, FontWeight::Thin))
            .or(args.thin.then_some(true)),
    });
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = loop {
        app.tick();
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && app.key(key) =>
                {
                    break Ok(())
                }
                Event::Key(_) => {}
                Event::Paste(text) => app.paste(&text),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
            }
        }
    };
    app.shutdown();
    terminal.show_cursor()?;
    result
}
