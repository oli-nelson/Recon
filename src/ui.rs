use crate::{
    Config, Theme,
    animation::{self, FramePacer},
    model::{Command, Model, Status},
    watch::Worker,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, IsTerminal},
    time::{Instant, UNIX_EPOCH},
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
    started: Option<Instant>,
    pub tree_percent: u16,
    arrivals: BTreeMap<String, Instant>,
    divider: Option<u16>,
    layout_x: u16,
    layout_width: u16,
    dragging: bool,
    split_area: Rect,
    pub search: String,
    pub searching: bool,
    search_backup: Option<SearchBackup>,
}
impl View {
    pub fn new() -> Self {
        Self {
            tree_percent: 32,
            started: Some(Instant::now()),
            sidebar: true,
            following: true,
            ..Self::default()
        }
    }
}
struct SearchBackup {
    query: String,
    selected: Option<String>,
    filter: Option<String>,
    scroll: usize,
    following: bool,
    focus: bool,
    tree_offset: usize,
}
#[derive(Clone)]
struct TreeRow {
    key: String,
    dir: bool,
    depth: usize,
}
fn tree(model: &Model, view: &View) -> Vec<TreeRow> {
    let mut nodes = BTreeSet::new();
    let query = view.search.to_lowercase();
    for path in model.files.keys() {
        if !query.is_empty() && !path.to_lowercase().contains(&query) {
            continue;
        }
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
        .filter(|(p, _)| {
            !view.search.is_empty() || !view.collapsed.iter().any(|d| p != d && p.starts_with(d))
        })
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
fn entry_matches(entry: &crate::model::Entry, view: &View) -> bool {
    view.filter.as_ref().is_none_or(|path| *path == entry.path)
        && (view.search.is_empty()
            || entry
                .path
                .to_lowercase()
                .contains(&view.search.to_lowercase()))
}

fn total(model: &Model, view: &View) -> usize {
    model
        .entries
        .iter()
        .filter(|e| entry_matches(e, view))
        .map(length)
        .sum()
}
fn save_anchor(model: &Model, view: &mut View) {
    let mut row = 0;
    for e in model
        .entries
        .iter()
        .rev()
        .filter(|e| entry_matches(e, view))
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
        .rev()
        .filter(|e| entry_matches(e, view))
    {
        if e.id == id {
            view.scroll = row + offset.min(length(e) - 1);
            return;
        }
        row += length(e);
    }
    // If the anchored entry was evicted, stay near the oldest retained content.
    view.scroll = total(model, view).saturating_sub(1);
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}
pub fn run(config: &Config, worker: Worker) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Interactive mode requires a terminal; use --once for a snapshot");
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut model = Model::new(
        config.history_entries as usize,
        config.history_bytes as usize,
    );
    let mut view = View::new();
    view.tree_percent = 100 - config.sidebar_width;
    let mut dirty = true;
    let mut pacer = FramePacer::new(config.fps, Instant::now());
    let mut was_active = false;
    let result = (|| -> Result<()> {
        loop {
            let before = model.entries.back().map(|e| e.id);
            save_anchor(&model, &mut view);
            let mut updated = false;
            for update in worker.updates.try_iter().take(16) {
                updated = true;
                if matches!(update, crate::model::Update::Reset { .. }) {
                    view.filter = None;
                    view.scroll = 0;
                    view.following = true;
                    view.anchor = None;
                    view.unseen = 0;
                    view.arrivals.clear();
                }
                if let crate::model::Update::Files(rows) = &update {
                    for (row, _) in rows {
                        if !config.no_animations
                            && !config.reduced_motion
                            && !row.remove_from_tree
                            && !model.files.contains_key(&row.path)
                        {
                            view.arrivals.insert(row.path.clone(), Instant::now());
                        }
                    }
                }
                model.apply(update);
                dirty = true;
            }
            if updated && !view.search.is_empty() {
                search_selection(&model, &mut view);
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
            let duration = std::time::Duration::from_millis(config.animation_ms);
            view.arrivals
                .retain(|path, start| start.elapsed() < duration && model.files.contains_key(path));
            let active = !config.no_animations
                && model
                    .files
                    .values()
                    .any(|r| r.activity.elapsed() < duration)
                || (!config.no_animations
                    && view.sidebar
                    && model
                        .entries
                        .back()
                        .is_some_and(|e| e.time.elapsed().is_ok_and(|age| age < duration)));
            let active = active || (!config.no_animations && !view.arrivals.is_empty());
            let animation =
                (active || model.refreshing) && !config.reduced_motion && !config.no_animations;
            if was_active && !active {
                dirty = true;
            }
            was_active = active;
            dirty |= animation;
            let now = Instant::now();
            if dirty && pacer.ready(now) {
                execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
                let rendered = terminal
                    .draw(|f| draw(f, &model, &mut view, config))
                    .map(|_| ());
                let ended = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
                rendered?;
                ended?;
                pacer.presented(now);
                dirty = false;
            }
            if event::poll(pacer.wait(Instant::now(), dirty || animation))? {
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
                    Event::Mouse(mouse) => {
                        dirty |= handle_mouse(mouse, &mut view);
                    }
                    Event::Resize(_, _) => {
                        view.dragging = false;
                        view.divider = None;
                        dirty = true;
                    }
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
fn resize_split(view: &mut View, percent: u16) {
    let width = view.layout_width.max(72) as u32;
    let min = (18 * 100u32).div_ceil(width).max(10) as u16;
    let max = (100 - (24 * 100u32).div_ceil(width)).min(90) as u16;
    view.tree_percent = percent.clamp(min, max);
}
fn handle_mouse(mouse: MouseEvent, view: &mut View) -> bool {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            view.dragging = view.sidebar
                && view.divider.is_some_and(|x| x.abs_diff(mouse.column) <= 1)
                && mouse.row >= view.split_area.y
                && mouse.row < view.split_area.bottom()
                && !view.help;
            view.dragging
        }
        MouseEventKind::Drag(MouseButton::Left) if view.dragging && view.divider.is_some() => {
            let relative = mouse.column.saturating_sub(view.layout_x) as u32;
            let percent = ((relative * 100) / view.layout_width.max(1) as u32).min(100) as u16;
            resize_split(view, percent);
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let was_dragging = view.dragging;
            view.dragging = false;
            was_dragging
        }
        _ => false,
    }
}

fn search_selection(model: &Model, view: &mut View) {
    let rows = tree(model, view);
    if view
        .selected
        .as_ref()
        .is_some_and(|selected| rows.iter().any(|r| !r.dir && &r.key == selected))
    {
        return;
    }
    if let Some(row) = rows.iter().find(|r| !r.dir) {
        select(view, row);
    } else {
        view.selected = None;
        view.filter = None;
    }
}

fn handle_key(
    key: KeyEvent,
    model: &mut Model,
    view: &mut View,
    commands: &std::sync::mpsc::SyncSender<Command>,
    height: usize,
) -> bool {
    use KeyCode::*;
    let page = height.saturating_sub(6).max(1);
    if view.searching {
        match key.code {
            Esc => {
                view.searching = false;
                if let Some(old) = view.search_backup.take() {
                    view.search = old.query;
                    view.selected = old.selected;
                    view.filter = old.filter;
                    view.scroll = old.scroll;
                    view.following = old.following;
                    view.panel_focus = old.focus;
                    view.tree_offset = old.tree_offset;
                }
            }
            Enter => {
                view.searching = false;
                view.search_backup = None;
            }
            Backspace => {
                view.search.pop();
                search_selection(model, view);
            }
            Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.search.clear();
                search_selection(model, view);
            }
            Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                view.search.push(c);
                search_selection(model, view);
            }
            _ => {}
        }
        return false;
    }
    if view.help {
        view.help = false;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if view.panel_focus && matches!(key.code, Char('d') | Char('u')) {
            view.following = false;
            let amount = page.div_ceil(2);
            if key.code == Char('d') {
                view.scroll = (view.scroll + amount).min(total(model, view).saturating_sub(page));
            } else {
                view.scroll = view.scroll.saturating_sub(amount);
            }
        }
        return false;
    }
    match key.code {
        Char('/') => {
            view.search_backup = Some(SearchBackup {
                query: view.search.clone(),
                selected: view.selected.clone(),
                filter: view.filter.clone(),
                scroll: view.scroll,
                following: view.following,
                focus: view.panel_focus,
                tree_offset: view.tree_offset,
            });
            view.searching = true;
            view.panel_focus = false;
        }
        Char('n') | Char('N') if !view.search.is_empty() => {
            let rows: Vec<_> = tree(model, view).into_iter().filter(|r| !r.dir).collect();
            if !rows.is_empty() {
                let current = rows
                    .iter()
                    .position(|r| Some(&r.key) == view.selected.as_ref())
                    .unwrap_or(0);
                let next = if key.code == Char('N') {
                    (current + rows.len() - 1) % rows.len()
                } else {
                    (current + 1) % rows.len()
                };
                select(view, &rows[next]);
            }
        }
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
            view.dragging = false;
            view.sidebar = !view.sidebar;
            view.divider = None;
            if !view.sidebar {
                view.panel_focus = false;
            } else if view.following {
                view.unseen = 0;
            }
        }
        Char('[') => resize_split(view, view.tree_percent.saturating_sub(5)),
        Char(']') => resize_split(view, view.tree_percent.saturating_add(5)),
        Tab if view.sidebar => view.panel_focus = !view.panel_focus,
        Char('?') => view.help = true,
        Esc => {
            view.search.clear();
            view.filter = None;
            view.following = true;
            view.scroll = 0;
            view.unseen = 0;
        }
        Char('f') => {
            view.following = !view.following;
            if view.following {
                view.unseen = 0;
            }
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
                .rev()
                .filter(|e| entry_matches(e, view))
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
                if key.code == Enter && !row.dir {
                    if view.filter.as_deref() != Some(&row.key) {
                        select(view, row);
                    }
                    view.sidebar = true;
                    view.panel_focus = true;
                } else if row.dir {
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
    let duration = std::time::Duration::from_millis(config.animation_ms);
    let bg = Color::Reset;
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
    let times: Vec<_> = model.modification_times.keys().copied().collect();
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
    let phase = view
        .started
        .map_or(0, |start| start.elapsed().as_millis() / 80);
    let indicator = if model.refreshing && !config.no_animations && !config.reduced_motion {
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][phase as usize % 10]
    } else {
        "●"
    };
    let mut counts = [0usize; 4];
    for file in model.files.values() {
        counts[match file.status {
            Status::New => 0,
            Status::Modified => 1,
            Status::Deleted => 2,
            Status::Unchanged => 3,
        }] += 1;
    }
    let positive = Color::Rgb(100, 210, 160);
    let negative = Color::Rgb(235, 122, 139);
    let gold = Color::Rgb(242, 195, 107);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " ◈ RECON ",
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  │ {indicator} {state}"),
                    Style::default().fg(if model.refreshing { gold } else { positive }),
                ),
                Span::styled(
                    format!("  │  {}", model.branch),
                    Style::default().fg(accent),
                ),
                Span::styled(
                    format!("  │  baseline {}", model.baseline),
                    Style::default().fg(muted),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("  +{} new  ", counts[0]),
                    Style::default().fg(positive),
                ),
                Span::styled(
                    format!("~{} modified  ", counts[1]),
                    Style::default().fg(gold),
                ),
                Span::styled(
                    format!("−{} deleted  ", counts[2]),
                    Style::default().fg(negative),
                ),
                Span::styled(
                    format!(
                        "·{} historical  │  {} entries · {:.1} MiB  │  {} FPS cap",
                        counts[3],
                        model.entries.len(),
                        model.bytes as f64 / 1048576.,
                        config.fps
                    ),
                    Style::default().fg(muted),
                ),
            ]),
        ]),
        parts[0],
    );
    let narrow = parts[1].width < 72;
    let tree_visible = !view.sidebar || !narrow || !view.panel_focus;
    let panel_visible = view.sidebar && (!narrow || view.panel_focus);
    view.split_area = parts[1];
    view.layout_x = parts[1].x;
    view.layout_width = parts[1].width;
    view.divider = None;
    let (tree_rect, panel_rect) = if tree_visible && panel_visible {
        let width = ((parts[1].width as u32 * view.tree_percent as u32) / 100) as u16;
        let width = width.clamp(18, parts[1].width.saturating_sub(24));
        let cols =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(24)]).split(parts[1]);
        view.divider = Some(cols[1].x);
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
                let indent = "│ ".repeat(r.depth);
                let name = r
                    .key
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&r.key);
                if r.dir {
                    ListItem::new(Line::from(vec![Span::styled(
                        format!(
                            "      {indent}{} {name}/",
                            if view.search.is_empty() && view.collapsed.contains(&r.key) {
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
                    let age = file.activity.elapsed();
                    let motion_age = view.arrivals.get(&file.path).map_or(
                        if file.status == Status::Deleted {
                            age
                        } else {
                            duration
                        },
                        Instant::elapsed,
                    );
                    let distance = tree_rect.width.saturating_sub(12 + (r.depth as u16) * 2);
                    let motion = animation::slide(
                        motion_age,
                        duration,
                        !config.no_animations && !config.reduced_motion,
                        file.status == Status::Deleted,
                        distance,
                    );
                    let active = age < duration && !config.no_animations;
                    let color = animation::glow(
                        color,
                        age,
                        duration,
                        !config.no_animations && !config.reduced_motion,
                        light,
                    );
                    let git_status: String = file.git.chars().take(2).collect();
                    let git_color = if git_status.contains('D') {
                        negative
                    } else if git_status == "??" || git_status.contains('A') {
                        positive
                    } else if git_status.trim().is_empty() {
                        muted
                    } else {
                        gold
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {git_status:2} "), Style::default().fg(git_color)),
                        Span::styled(
                            "● ",
                            Style::default().fg(animation::recency(
                                animation::relative_age(file.modified_at, &times),
                                light,
                                muted,
                            )),
                        ),
                        Span::styled(
                            format!(
                                "{indent}{}{} {name}",
                                " ".repeat((2 + motion).max(0) as usize),
                                file.status.symbol()
                            ),
                            Style::default().fg(color).add_modifier(if active {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                        ),
                        Span::styled(
                            file.git.get(2..).unwrap_or("").to_owned(),
                            Style::default().fg(muted),
                        ),
                    ]))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Files ")
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if !view.panel_focus { accent } else { muted }));
        let list = List::new(items)
            .highlight_symbol("▸ ")
            .highlight_spacing(HighlightSpacing::Always)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
        let mut state = ListState::default()
            .with_offset(view.tree_offset)
            .with_selected(selected);
        frame.render_stateful_widget(list, tree_rect, &mut state);
        view.tree_offset = state.offset();
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new(if view.search.is_empty() {
                    "\n  No changes yet.\n  Edits will appear here."
                } else {
                    "\n  No matching files.\n  Esc clears the search."
                })
                .style(Style::default().fg(muted)),
                inset(tree_rect),
            );
        }
    }
    if panel_visible {
        let label = view.filter.as_deref().unwrap_or(if view.search.is_empty() {
            "All files"
        } else {
            "Matching files"
        });
        let title = format!(
            " History • {label} • {} {} ",
            if view.following {
                "LIVE"
            } else {
                "PAUSED • f to resume"
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
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if view.panel_focus || view.dragging {
                accent
            } else {
                muted
            }));
        let inner = block.inner(panel_rect);
        frame.render_widget(block, panel_rect);
        let height = inner.height as usize;
        let count = total(model, view);
        if view.following {
            view.scroll = 0;
        }
        view.scroll = view.scroll.min(count.saturating_sub(height));
        let mut lines = Vec::new();
        let mut row = 0;
        for entry in model
            .entries
            .iter()
            .rev()
            .filter(|e| entry_matches(e, view))
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
            let age = entry.time.elapsed().unwrap_or(duration);
            let shift = " ".repeat(
                (2 + animation::slide(
                    age,
                    duration,
                    !config.no_animations && !config.reduced_motion,
                    false,
                    inner.width.saturating_sub(4),
                ))
                .max(0) as usize,
            );
            let header_color = animation::glow(
                accent,
                age,
                duration,
                !config.no_animations && !config.reduced_motion,
                light,
            );
            let header = format!(
                "{shift}╭─ {} {}  {}  • {}",
                if entry.collapsed { "▸" } else { "▾" },
                entry.path,
                time,
                entry.activity
            );
            let mut local = Vec::new();
            local.push(Line::styled(
                header,
                Style::default()
                    .fg(header_color)
                    .add_modifier(Modifier::BOLD),
            ));
            if entry.collapsed {
                local.push(Line::styled(
                    format!("{shift}╰─ {} lines • Enter to expand", entry.line_count),
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
                            Line::from(vec![
                                Span::raw(shift.clone()),
                                Span::styled("│ ", Style::default().fg(muted)),
                                Span::styled(line.to_owned(), Style::default().fg(color)),
                            ]),
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
            if !entry.collapsed {
                let bottom = row + n - 1;
                if bottom >= view.scroll && bottom < view.scroll + height {
                    lines.push((
                        bottom - view.scroll,
                        Line::styled(
                            format!("{shift}╰────────────────"),
                            Style::default().fg(muted),
                        ),
                    ));
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
        " / search  q quit  ? help  d panel  Tab focus  [ ] resize  c clear  g Git  Esc all  f live{}",
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
            if view.searching || !view.search.is_empty() {
                Line::from(vec![
                    Span::styled(
                        format!(" /{}{}", view.search, if view.searching { "▏" } else { "" }),
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        if view.searching {
                            "  Enter keep · Esc cancel · Ctrl-U clear"
                        } else {
                            "  n/N next/previous · Esc clear filter"
                        },
                        Style::default().fg(muted),
                    ),
                ])
            } else {
                Line::styled(status, Style::default().fg(accent))
            },
        ]),
        parts[2],
    );
    if view.help {
        let area = frame.area();
        let popup = Rect::new(
            area.width.saturating_sub(66) / 2,
            area.height.saturating_sub(24) / 2,
            area.width.min(66),
            area.height.min(24),
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new("\n /  Search file paths; Enter keep, Esc cancel\n c  Clear history; use current contents as baseline\n g  Fresh snapshot against HEAD\n d  Hide/show panel (history continues)\n Tab  Switch focus / narrow-terminal view\n j/k or arrows  Navigate files / scroll history\n h/l or arrows  Collapse/expand directories\n [ / ] or drag divider  Resize sidebar horizontally\n Enter  Open/focus file history; fold history entries\n Ctrl-D / Ctrl-U  Half-page down/up in sidebar\n PageUp/PageDown  Full-page down/up\n f  Toggle live following\n Esc  All-file history\n ?  Help     q  Quit\n\n ● relative mtime: cyan → blue → amber → muted\n Git status: index/worktree; ?? untracked\n Recon never writes to your repository.").block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Recon • keyboard ")).style(Style::default().bg(bg).fg(fg)), popup);
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
                    remove_from_tree: false,
                    modified_at: None,
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
        assert_eq!(v.anchor, Some((4, 0)));
        m.entries.pop_front();
        restore_anchor(&m, &mut v);
        assert_eq!(v.scroll, 12);
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
    #[test]
    fn search_matches_collapsed_paths_and_supports_accept_cancel_and_next() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.collapsed.insert("dir/".into());
        view.sidebar = false;
        handle_key(KeyCode::Char('/').into(), &mut model, &mut view, &tx, 24);
        for c in "FILE3".chars() {
            handle_key(KeyCode::Char(c).into(), &mut model, &mut view, &tx, 24);
        }
        let matches: Vec<_> = tree(&model, &view).into_iter().filter(|r| !r.dir).collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].key, "dir/file3.rs");
        assert!(!view.sidebar);
        assert_eq!(view.filter.as_deref(), Some("dir/file3.rs"));
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(!view.searching);
        handle_key(KeyCode::Char('/').into(), &mut model, &mut view, &tx, 24);
        handle_key(KeyCode::Char('x').into(), &mut model, &mut view, &tx, 24);
        assert!(tree(&model, &view).is_empty());
        assert_eq!(total(&model, &view), 0);
        handle_key(KeyCode::Esc.into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.search, "FILE3");
        assert_eq!(view.filter.as_deref(), Some("dir/file3.rs"));
        handle_key(KeyCode::Esc.into(), &mut model, &mut view, &tx, 24);
        assert!(view.search.is_empty());
        assert!(view.filter.is_none());
        view.collapsed.clear();
        view.search = "file".into();
        search_selection(&model, &mut view);
        handle_key(KeyCode::Char('n').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.selected.as_deref(), Some("dir/file4.rs"));
        handle_key(KeyCode::Char('N').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.selected.as_deref(), Some("dir/file3.rs"));
    }
    #[test]
    fn divider_drag_and_keys_preserve_width_and_bound_both_panes() {
        let mut model = history_model();
        let mut view = View::new();
        let config = Config::parse_from(["recon"]);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let divider = view.divider.unwrap();
        let mouse = |kind, x| MouseEvent {
            kind,
            column: x,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), divider),
            &mut view
        ));
        assert!(handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 72),
            &mut view
        ));
        assert_eq!(view.tree_percent, 60);
        handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 72), &mut view);
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Char('[').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.tree_percent, 55);
        handle_key(KeyCode::Char('d').into(), &mut model, &mut view, &tx, 24);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert!(view.divider.is_none());
        handle_key(KeyCode::Char('d').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.tree_percent, 55);
        view.tree_percent = 90;
        let mut narrow = Terminal::new(TestBackend::new(72, 24)).unwrap();
        narrow
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert!(view.divider.unwrap() <= 48);
        assert_eq!(view.tree_percent, 90);
        let mut tiny = Terminal::new(TestBackend::new(30, 10)).unwrap();
        tiny.draw(|f| draw(f, &model, &mut view, &config)).unwrap();
        assert!(view.divider.is_none());
    }
    #[test]
    fn status_column_stays_aligned_during_motion_at_different_depths() {
        let mut model = history_model();
        let mut view = View::new();
        let mut root = model.files["dir/file0.rs"].clone();
        root.path = "rootfile.rs".into();
        model.files.insert(root.path.clone(), root);
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        let config = Config::parse_from(["recon"]);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let width = view.divider.unwrap() as usize;
        let columns: Vec<_> = terminal
            .backend()
            .buffer()
            .content
            .chunks(140)
            .map(|row| {
                row.iter()
                    .take(width)
                    .map(|c| c.symbol())
                    .collect::<String>()
            })
            .filter(|row| row.contains("file") && row.contains('M'))
            .map(|row| row.chars().position(|c| c == 'M').unwrap())
            .collect();
        assert!(columns.len() >= 2);
        assert!(columns.iter().all(|x| *x == columns[0]));
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.bg == Color::Reset)
        );
    }
    #[test]
    fn enter_on_file_opens_and_focuses_sidebar_without_collapsing_parent() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.selected = Some("dir/file3.rs".into());
        view.filter = view.selected.clone();
        view.sidebar = false;
        view.scroll = 2;
        view.following = false;
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(view.sidebar && view.panel_focus);
        assert!(!view.collapsed.contains("dir/"));
        assert_eq!(view.selected.as_deref(), Some("dir/file3.rs"));
        assert_eq!(view.scroll, 2);
        assert!(!view.following);
        view.panel_focus = false;
        view.selected = Some("dir/".into());
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(view.collapsed.contains("dir/"));
    }
    #[test]
    fn control_scrolls_half_pages_and_f_toggles_live_without_hiding_sidebar() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.panel_focus = true;
        view.scroll = 14;
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        handle_key(ctrl('u'), &mut model, &mut view, &tx, 24);
        assert_eq!(view.scroll, 5);
        assert!(!view.following);
        assert!(view.sidebar);
        handle_key(ctrl('d'), &mut model, &mut view, &tx, 24);
        assert_eq!(view.scroll, 14);
        assert!(view.sidebar);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        assert!(view.following);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        assert!(!view.following);
        view.panel_focus = false;
        handle_key(ctrl('d'), &mut model, &mut view, &tx, 24);
        assert!(view.sidebar);
    }
    #[test]
    fn spring_entrance_is_clipped_then_overshoots_without_moving_status() {
        let model = history_model();
        let config = Config::parse_from(["recon"]);
        let mut view = View::new();
        view.sidebar = false;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let line = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .chunks(100)
                .nth(4)
                .unwrap()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        view.arrivals.insert("dir/file0.rs".into(), Instant::now());
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let initial = line(&terminal);
        assert!(!initial.contains("file0.rs"));
        assert!(initial.contains('M'));
        view.arrivals.insert(
            "dir/file0.rs".into(),
            Instant::now() - std::time::Duration::from_millis(400),
        );
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let spring = line(&terminal);
        view.arrivals.clear();
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let settled = line(&terminal);
        assert!(spring.find("file0.rs").unwrap() < settled.find("file0.rs").unwrap());
        assert_eq!(initial.find('M'), settled.find('M'));
        assert_eq!(spring.find('M'), settled.find('M'));
    }
    #[test]
    fn newest_entries_are_at_top_and_prepending_preserves_paused_reading() {
        let mut model = history_model();
        model.refreshing = false;
        model.branch = "feature/top-first".into();
        let mut view = View::new();
        view.following = false;
        view.panel_focus = true;
        view.scroll = 4;
        save_anchor(&model, &mut view);
        assert_eq!(view.anchor, Some((6, 0)));
        let mut entry = model.entries.back().unwrap().clone();
        entry.id = 8;
        entry.path = "dir/newest.rs".into();
        let mut row = model.files["dir/file7.rs"].clone();
        row.path = entry.path.clone();
        model.apply(crate::model::Update::Files(vec![(row, Some(entry))]));
        restore_anchor(&model, &mut view);
        assert_eq!(view.scroll, 8);
        save_anchor(&model, &mut view);
        assert_eq!(view.anchor, Some((6, 0)));
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert_eq!(view.scroll, 0);
        let header = terminal
            .backend()
            .buffer()
            .content
            .chunks(140)
            .nth(3)
            .unwrap()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(header.contains("dir/newest.rs"));
        let top = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .take(140)
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(top.find("WATCHING").unwrap() < top.find("feature/top-first").unwrap());
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(model.entries.back().unwrap().collapsed);
        assert!(!model.entries.front().unwrap().collapsed);
    }
}
