use crate::{
    Config, Theme,
    model::{Command, Model, Status},
    watch::Worker,
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::{
    collections::BTreeSet,
    io::{self, IsTerminal},
    time::{Duration, Instant, UNIX_EPOCH},
};

#[derive(Default)]
pub struct View {
    pub filter: Option<String>,
    pub selected: Option<String>,
    pub collapsed: BTreeSet<String>,
    pub sidebar: bool,
    pub panel_focus: bool,
    pub following: bool,
    pub scroll: usize,
    pub cursor: usize,
    pub help: bool,
    pub unseen: usize,
    pub tree_offset: usize,
    anchor: Option<(u64, usize)>,
}
impl View {
    pub fn new() -> Self {
        Self {
            sidebar: true,
            following: true,
            ..Self::default()
        }
    }
}
#[derive(Clone)]
struct TreeRow {
    key: String,
    dir: bool,
    depth: usize,
}
fn tree(model: &Model, view: &View) -> Vec<TreeRow> {
    let mut nodes = BTreeSet::new();
    for path in model.files.keys() {
        let mut dir = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(p);
            nodes.insert((format!("{dir}/"), true));
        }
        nodes.insert((path.clone(), false));
    }
    nodes
        .into_iter()
        .filter(|(p, _)| !view.collapsed.iter().any(|d| p != d && p.starts_with(d)))
        .map(|(key, dir)| TreeRow {
            depth: key.trim_end_matches('/').matches('/').count(),
            key,
            dir,
        })
        .collect()
}
fn length(entry: &crate::model::Entry) -> usize {
    if entry.collapsed {
        2
    } else {
        2 + entry.line_count.max(1)
    }
}
fn total(model: &Model, view: &View) -> usize {
    model
        .entries
        .iter()
        .filter(|e| view.filter.as_ref().is_none_or(|p| *p == e.path))
        .map(length)
        .sum()
}
fn save_anchor(model: &Model, view: &mut View) {
    let mut row = 0;
    for e in model
        .entries
        .iter()
        .filter(|e| view.filter.as_ref().is_none_or(|p| *p == e.path))
    {
        let n = length(e);
        if view.scroll < row + n {
            view.anchor = Some((e.id, view.scroll - row));
            return;
        }
        row += n;
    }
    view.anchor = None;
}
fn restore_anchor(model: &Model, view: &mut View) {
    let Some((id, offset)) = view.anchor else {
        return;
    };
    let mut row = 0;
    for e in model
        .entries
        .iter()
        .filter(|e| view.filter.as_ref().is_none_or(|p| *p == e.path))
    {
        if e.id == id {
            view.scroll = row + offset.min(length(e) - 1);
            return;
        }
        row += length(e);
    }
    view.scroll = 0;
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
pub fn run(config: &Config, worker: Worker) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Interactive mode requires a terminal; use --once for a snapshot");
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut model = Model::new(
        config.history_entries as usize,
        config.history_bytes as usize,
    );
    let mut view = View::new();
    let mut dirty = true;
    let mut last_tick = Instant::now();
    let mut was_active = false;
    let result = (|| -> Result<()> {
        loop {
            let before = model.entries.back().map(|e| e.id);
            save_anchor(&model, &mut view);
            for update in worker.updates.try_iter().take(16) {
                if matches!(update, crate::model::Update::Reset { .. }) {
                    view.filter = None;
                    view.scroll = 0;
                    view.following = true;
                    view.anchor = None;
                    view.unseen = 0;
                }
                model.apply(update);
                dirty = true;
            }
            if !view.following {
                restore_anchor(&model, &mut view);
            }
            let added = model.entries.back().map_or(0, |entry| {
                before.map_or(model.entries.len(), |id| {
                    entry.id.saturating_sub(id) as usize
                })
            });
            if !view.following || !view.sidebar {
                view.unseen += added;
            }
            let active = !config.no_animations
                && model
                    .files
                    .values()
                    .any(|r| r.activity.elapsed() < Duration::from_millis(400));
            let animation = active && !config.reduced_motion;
            if was_active && !active {
                dirty = true;
            }
            was_active = active;
            if animation && last_tick.elapsed() >= Duration::from_millis(33) {
                dirty = true;
                last_tick = Instant::now();
            }
            if dirty {
                terminal.draw(|f| draw(f, &model, &mut view, config))?;
                dirty = false;
            }
            if event::poll(if animation {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(40)
            })? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if handle_key(
                            key,
                            &mut model,
                            &mut view,
                            &worker.commands,
                            terminal.size()?.height as usize,
                        ) {
                            break;
                        }
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    worker.stop();
    terminal.show_cursor()?;
    result
}
fn handle_key(
    key: KeyEvent,
    model: &mut Model,
    view: &mut View,
    commands: &std::sync::mpsc::SyncSender<Command>,
    height: usize,
) -> bool {
    use KeyCode::*;
    let page = height.saturating_sub(8).max(1);
    if view.help {
        view.help = false;
        return false;
    }
    match key.code {
        Char('q') => return true,
        Char('c') | Char('g') => {
            let command = if key.code == Char('c') {
                Command::Clear
            } else {
                Command::Git
            };
            if commands.try_send(command).is_ok() {
                model.refreshing = true;
                model.message = "Requested baseline refresh…".into();
            } else {
                model.message = "Refresh queue busy; retry shortly".into();
            }
        }
        Char('d') => {
            view.sidebar = !view.sidebar;
            if !view.sidebar {
                view.panel_focus = false;
            } else if view.following {
                view.unseen = 0;
            }
        }
        Tab if view.sidebar => view.panel_focus = !view.panel_focus,
        Char('?') => view.help = true,
        Esc => {
            view.filter = None;
            view.following = true;
            view.scroll = 0;
            view.unseen = 0;
        }
        End => {
            view.following = true;
            view.unseen = 0;
        }
        PageUp | PageDown => {
            view.following = false;
            if key.code == PageUp {
                view.scroll = view.scroll.saturating_sub(page);
            } else {
                view.scroll = (view.scroll + page).min(total(model, view).saturating_sub(page));
            }
        }
        Enter if view.panel_focus => {
            let mut offset = 0;
            for e in model
                .entries
                .iter_mut()
                .filter(|e| view.filter.as_ref().is_none_or(|p| *p == e.path))
            {
                let len = length(e);
                if view.scroll + view.cursor < offset + len {
                    e.collapsed = !e.collapsed;
                    view.following = false;
                    break;
                }
                offset += len;
            }
        }
        Char('j') | Down | Char('k') | Up => {
            let down = matches!(key.code, Char('j') | Down);
            if view.panel_focus {
                view.following = false;
                if down {
                    view.scroll = (view.scroll + 1).min(total(model, view).saturating_sub(page));
                } else {
                    view.scroll = view.scroll.saturating_sub(1);
                }
            } else {
                let rows = tree(model, view);
                if !rows.is_empty() {
                    let index = view
                        .selected
                        .as_ref()
                        .and_then(|p| rows.iter().position(|r| &r.key == p))
                        .unwrap_or(if down { usize::MAX } else { 0 });
                    let index = if down {
                        if index == usize::MAX {
                            0
                        } else {
                            (index + 1).min(rows.len() - 1)
                        }
                    } else {
                        index.saturating_sub(1)
                    };
                    select(view, &rows[index]);
                }
            }
        }
        Char('h') | Left | Char('l') | Right | Enter => {
            let rows = tree(model, view);
            if let Some(index) = view
                .selected
                .as_ref()
                .and_then(|p| rows.iter().position(|r| &r.key == p))
            {
                let row = &rows[index];
                let expand = matches!(key.code, Char('l') | Right)
                    || key.code == Enter && view.collapsed.contains(&row.key);
                if row.dir {
                    if expand {
                        view.collapsed.remove(&row.key);
                    } else {
                        view.collapsed.insert(row.key.clone());
                    }
                } else if !expand {
                    if let Some((parent, _)) = row.key.rsplit_once('/') {
                        view.selected = Some(format!("{parent}/"));
                        view.collapsed.insert(format!("{parent}/"));
                    }
                }
            }
        }
        _ => {}
    }
    false
}
fn select(view: &mut View, row: &TreeRow) {
    view.selected = Some(row.key.clone());
    if !row.dir {
        view.filter = Some(row.key.clone());
        view.following = true;
        view.scroll = 0;
        view.unseen = 0;
    }
}

pub fn draw(frame: &mut Frame, model: &Model, view: &mut View, config: &Config) {
    let light = matches!(config.theme, Theme::Light);
    let bg = if light {
        Color::Rgb(244, 246, 250)
    } else {
        Color::Rgb(15, 18, 27)
    };
    let fg = if light {
        Color::Rgb(35, 43, 60)
    } else {
        Color::Rgb(204, 214, 237)
    };
    let muted = if light {
        Color::Rgb(100, 110, 128)
    } else {
        Color::Rgb(113, 128, 155)
    };
    let accent = Color::Rgb(106, 173, 255);
    frame.render_widget(
        Block::default().style(Style::default().bg(bg).fg(fg)),
        frame.area(),
    );
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let state = if model.refreshing {
        "REFRESHING"
    } else if model.watch_status.is_some() {
        "RECOVERY ONLY"
    } else {
        "WATCHING"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " RECON ",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {state}  •  baseline: {}  •  {} files  •  {} entries",
                model.baseline,
                model.files.len(),
                model.entries.len()
            )),
        ])),
        parts[0],
    );
    let narrow = parts[1].width < 72;
    let tree_visible = !view.sidebar || !narrow || !view.panel_focus;
    let panel_visible = view.sidebar && (!narrow || view.panel_focus);
    let (tree_rect, panel_rect) = if tree_visible && panel_visible {
        let cols = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(parts[1]);
        (cols[0], cols[1])
    } else {
        (parts[1], parts[1])
    };
    if tree_visible {
        let rows = tree(model, view);
        let selected = view
            .selected
            .as_ref()
            .and_then(|p| rows.iter().position(|r| &r.key == p));
        let items: Vec<_> = rows
            .iter()
            .map(|r| {
                let indent = "  ".repeat(r.depth);
                let name = r
                    .key
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&r.key);
                if r.dir {
                    ListItem::new(Line::from(vec![Span::styled(
                        format!(
                            " {indent}{} {name}/",
                            if view.collapsed.contains(&r.key) {
                                "▸"
                            } else {
                                "▾"
                            }
                        ),
                        Style::default().fg(muted),
                    )]))
                } else {
                    let file = &model.files[&r.key];
                    let color = match file.status {
                        Status::New => Color::Rgb(100, 210, 160),
                        Status::Modified => Color::Rgb(242, 195, 107),
                        Status::Deleted | Status::Unchanged => muted,
                    };
                    let elapsed = file.activity.elapsed().as_millis();
                    let motion = if config.no_animations || config.reduced_motion || elapsed >= 400
                    {
                        0
                    } else if file.status == Status::Deleted {
                        (elapsed / 100) as usize
                    } else {
                        3usize.saturating_sub((elapsed / 100) as usize)
                    };
                    let active = elapsed < 400 && !config.no_animations;
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!(
                                " {indent}{}{} {name}",
                                " ".repeat(motion),
                                file.status.symbol()
                            ),
                            Style::default().fg(color).add_modifier(if active {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                        ),
                        Span::styled(format!("  [{}]", file.git), Style::default().fg(muted)),
                    ]))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Files  + new  ~ modified  − deleted  · unchanged ")
            .border_style(Style::default().fg(if !view.panel_focus { accent } else { muted }));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(if light {
                Color::Rgb(213, 226, 245)
            } else {
                Color::Rgb(34, 46, 68)
            }));
        let mut state = ListState::default()
            .with_offset(view.tree_offset)
            .with_selected(selected);
        frame.render_stateful_widget(list, tree_rect, &mut state);
        view.tree_offset = state.offset();
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new("\n  No changes yet.\n  Edits will appear here.")
                    .style(Style::default().fg(muted)),
                inset(tree_rect),
            );
        }
    }
    if panel_visible {
        let label = view.filter.as_deref().unwrap_or("All files");
        let title = format!(
            " History • {label} • {} {} ",
            if view.following {
                "LIVE"
            } else {
                "PAUSED • End to resume"
            },
            if view.unseen > 0 {
                format!("• {} new", view.unseen)
            } else {
                String::new()
            }
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(if view.panel_focus { accent } else { muted }));
        let inner = block.inner(panel_rect);
        frame.render_widget(block, panel_rect);
        let height = inner.height as usize;
        let count = total(model, view);
        if view.following {
            view.scroll = count.saturating_sub(height);
        }
        view.scroll = view.scroll.min(count.saturating_sub(height));
        let mut lines = Vec::new();
        let mut row = 0;
        for entry in model
            .entries
            .iter()
            .filter(|e| view.filter.as_ref().is_none_or(|p| *p == e.path))
        {
            let n = length(entry);
            if row + n <= view.scroll {
                row += n;
                continue;
            }
            if row >= view.scroll + height {
                break;
            }
            let secs = entry
                .time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                % 86400;
            let time = format!(
                "{:02}:{:02}:{:02}Z",
                secs / 3600,
                secs % 3600 / 60,
                secs % 60
            );
            let age = entry.time.elapsed().unwrap_or_default().as_millis();
            let shift = if !config.no_animations && !config.reduced_motion && age < 400 {
                " ".repeat(3usize.saturating_sub((age / 100) as usize))
            } else {
                String::new()
            };
            let header = format!(
                "{shift}{} {}  {}  • {}",
                if entry.collapsed { "▸" } else { "▾" },
                entry.path,
                time,
                entry.activity
            );
            let mut local = Vec::new();
            local.push(Line::styled(
                header,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ));
            if entry.collapsed {
                local.push(Line::styled(
                    format!("  {} lines • Enter to expand", entry.line_count),
                    Style::default().fg(muted),
                ));
            } else {
                // Only materialize visible lines, even for very large retained entries.
                let from = view.scroll.saturating_sub(row).saturating_sub(1);
                for (i, line) in entry.diff.lines().enumerate().skip(from).take(height + 1) {
                    let color = if line.starts_with('+') {
                        Color::Rgb(100, 210, 160)
                    } else if line.starts_with('-') {
                        Color::Rgb(235, 122, 139)
                    } else if line.starts_with("@@") {
                        accent
                    } else {
                        fg
                    };
                    // Put lines directly by their virtual row (no allocation for offscreen text).
                    let absolute = row + i + 1;
                    if absolute >= view.scroll && absolute < view.scroll + height {
                        lines.push((
                            absolute - view.scroll,
                            Line::styled(line.to_owned(), Style::default().fg(color)),
                        ));
                    }
                }
            }
            for (i, line) in local.into_iter().enumerate() {
                let absolute = row + i;
                if absolute >= view.scroll && absolute < view.scroll + height {
                    lines.push((absolute - view.scroll, line));
                }
            }
            row += n;
        }
        for (y, line) in lines {
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(inner.x, inner.y + y as u16, inner.width, 1),
            );
        }
        if count == 0 {
            frame.render_widget(
                Paragraph::new(
                    "\n  Waiting for observed changes…\n\n  c clear baseline   g Git snapshot",
                )
                .style(Style::default().fg(muted)),
                inner,
            );
        }
    }
    let footer = format!(
        " q quit  ? help  d panel  Tab focus  c clear  g Git  Esc all  End live{}",
        if narrow { " • Tab switches view" } else { "" }
    );
    let status = format!(
        " {}{}{}",
        if let Some(warning) = &model.watch_status {
            warning
        } else if model.refreshing {
            &model.message
        } else {
            &model.notice
        },
        if model.evicted > 0 {
            format!(" • {} oldest entries evicted", model.evicted)
        } else {
            String::new()
        },
        if !view.sidebar && view.unseen > 0 {
            format!(" • {} updates while hidden", view.unseen)
        } else {
            String::new()
        }
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(footer, Style::default().fg(muted)),
            Line::styled(status, Style::default().fg(accent)),
        ]),
        parts[2],
    );
    if view.help {
        let area = frame.area();
        let popup = Rect::new(
            area.width.saturating_sub(66) / 2,
            area.height.saturating_sub(18) / 2,
            area.width.min(66),
            area.height.min(18),
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new("\n c  Clear history; use current contents as baseline\n g  Fresh snapshot against HEAD\n d  Hide/show panel (history continues)\n Tab  Switch focus / narrow-terminal view\n j/k or arrows  Navigate files / scroll history\n h/l or arrows  Collapse/expand directories\n Enter  Collapse/expand history entry at top of panel\n PageUp/PageDown  Scroll; pause live following\n End  Return to live end\n Esc  All-file history\n ?  Help     q  Quit\n\n Git status: [index worktree], ?? untracked\n Recon never writes to your repository.").block(Block::default().borders(Borders::ALL).title(" Recon • keyboard ")).style(Style::default().bg(bg).fg(fg)), popup);
    }
}
fn inset(r: Rect) -> Rect {
    Rect::new(
        r.x.saturating_add(1),
        r.y.saturating_add(1),
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use ratatui::backend::TestBackend;
    #[test]
    fn layouts_render_at_tiny_and_wide_sizes() {
        let config = Config::parse_from(["recon"]);
        for (w, h) in [(1, 1), (30, 10), (80, 24), (140, 40)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            let m = Model::new(100, 4096);
            let mut v = View::new();
            for panel in [false, true] {
                v.panel_focus = panel;
                term.draw(|f| draw(f, &m, &mut v, &config)).unwrap();
            }
        }
    }
    fn history_model() -> Model {
        let mut model = Model::new(100, 1_000_000);
        let mut rows = Vec::new();
        for id in 0..8 {
            let path = format!("dir/file{id}.rs");
            rows.push((
                crate::model::FileRow {
                    path: path.clone(),
                    status: Status::Modified,
                    git: " M".into(),
                    note: String::new(),
                    activity: Instant::now(),
                },
                Some(crate::model::Entry {
                    id,
                    path,
                    time: std::time::SystemTime::now(),
                    activity: "edited".into(),
                    diff: "-before\n+after".into(),
                    collapsed: false,
                    line_count: 2,
                }),
            ));
        }
        model.apply(crate::model::Update::Files(rows));
        model
    }
    #[test]
    fn sidebar_preserves_filter_scroll_and_focus_rules() {
        let mut m = history_model();
        let mut v = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        v.filter = Some("dir/file0.rs".into());
        v.scroll = 2;
        v.following = false;
        v.panel_focus = true;
        handle_key(KeyCode::Char('d').into(), &mut m, &mut v, &tx, 24);
        assert!(!v.sidebar);
        assert!(!v.panel_focus);
        handle_key(KeyCode::Down.into(), &mut m, &mut v, &tx, 24);
        assert!(!v.sidebar); // Navigating a hidden sidebar never reopens it.
        v.filter = Some("dir/file0.rs".into());
        v.scroll = 2;
        v.following = false;
        handle_key(KeyCode::Char('d').into(), &mut m, &mut v, &tx, 24);
        assert!(v.sidebar);
        assert_eq!(v.scroll, 2);
        assert_eq!(v.filter.as_deref(), Some("dir/file0.rs"));
    }
    #[test]
    fn reading_anchor_survives_eviction_and_tree_identity_survives_insertion() {
        let mut m = history_model();
        let mut v = View::new();
        v.scroll = 12;
        v.following = false;
        v.selected = Some("dir/file5.rs".into());
        save_anchor(&m, &mut v);
        assert_eq!(v.anchor, Some((3, 0)));
        m.entries.pop_front();
        restore_anchor(&m, &mut v);
        assert_eq!(v.scroll, 8);
        let mut row = m.files["dir/file5.rs"].clone();
        row.path = "aaa".into();
        m.files.insert(row.path.clone(), row);
        assert!(
            tree(&m, &v)
                .iter()
                .any(|r| Some(&r.key) == v.selected.as_ref())
        );
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Left.into(), &mut m, &mut v, &tx, 24);
        assert!(v.collapsed.contains("dir/"));
        assert_eq!(v.selected.as_deref(), Some("dir/"));
        handle_key(KeyCode::Right.into(), &mut m, &mut v, &tx, 24);
        assert!(!v.collapsed.contains("dir/"));
    }
}
