use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

const NOTICE_LIFETIME: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    New,
    Modified,
    Deleted,
    Unchanged,
}
impl Status {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::New => "+",
            Self::Modified => "~",
            Self::Deleted => "−",
            Self::Unchanged => "·",
        }
    }
}
#[derive(Debug, Clone)]
pub struct FileRow {
    pub path: String,
    pub status: Status,
    pub git: String,
    pub note: String,
    pub activity: Instant,
    pub remove_from_tree: bool,
    pub modified_at: Option<SystemTime>,
}
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    pub path: String,
    pub time: SystemTime,
    pub activity: String,
    pub diff: Arc<str>,
    pub collapsed: bool,
    pub line_count: usize,
}
impl Entry {
    pub fn bytes(&self) -> usize {
        self.path.len() + self.diff.len() + self.activity.len() + std::mem::size_of::<Self>()
    }
}
#[derive(Debug)]
pub enum Update {
    Reset {
        root: PathBuf,
        baseline: String,
        reason: String,
        branch: String,
    },
    Files(Vec<(FileRow, Option<Entry>)>),
    Progress(String),
    Notice(String),
    WatchStatus(String),
    Done,
}
#[derive(Debug, Clone, Copy)]
pub enum Command {
    Clear,
    Git,
    Reconcile,
    Quit,
}

pub struct Model {
    pub root: PathBuf,
    pub files: BTreeMap<String, FileRow>,
    pub entries: VecDeque<Entry>,
    pub bytes: usize,
    pub modification_times: BTreeMap<SystemTime, usize>,
    pub evicted: usize,
    pub baseline: String,
    pub branch: String,
    pub message: String,
    pub refreshing: bool,
    pub notice: String,
    notice_until: Option<Instant>,
    pub watch_status: Option<String>,
    limit_entries: usize,
    limit_bytes: usize,
}
impl Model {
    pub fn new(entries: usize, bytes: usize) -> Self {
        Self {
            root: PathBuf::new(),
            files: BTreeMap::new(),
            entries: VecDeque::new(),
            bytes: 0,
            modification_times: BTreeMap::new(),
            evicted: 0,
            baseline: "HEAD".into(),
            branch: "…".into(),
            message: "Reading repository…".into(),
            refreshing: true,
            notice: String::new(),
            notice_until: None,
            watch_status: None,
            limit_entries: entries,
            limit_bytes: bytes,
        }
    }
    fn set_notice(&mut self, message: String) {
        self.notice_until = (!message.is_empty()).then(|| Instant::now() + NOTICE_LIFETIME);
        self.notice = message;
    }

    pub fn dismiss_notice(&mut self) -> bool {
        let visible = !self.notice.is_empty();
        self.notice.clear();
        self.notice_until = None;
        visible
    }

    pub fn notice_remaining(&self, now: Instant) -> Option<f64> {
        if self.refreshing {
            return None;
        }
        self.notice_until.map(|deadline| {
            (deadline.saturating_duration_since(now).as_secs_f64() / NOTICE_LIFETIME.as_secs_f64())
                .clamp(0.0, 1.0)
        })
    }

    pub fn expire_notice(&mut self, now: Instant) -> bool {
        if !self.refreshing && self.notice_until.is_some_and(|deadline| now >= deadline) {
            return self.dismiss_notice();
        }
        false
    }

    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Reset {
                root,
                baseline,
                reason,
                branch,
            } => {
                self.root = root;
                self.branch = branch;
                self.files.clear();
                self.entries.clear();
                self.bytes = 0;
                self.modification_times.clear();
                self.evicted = 0;
                self.baseline = baseline;
                self.set_notice(reason.clone());
                self.message = reason;
                self.refreshing = true;
            }
            Update::Files(rows) => {
                for (mut row, entry) in rows {
                    if entry.is_none() {
                        if let Some(previous) = self.files.get(&row.path) {
                            if row.modified_at.is_none() {
                                row.modified_at = previous.modified_at;
                            }
                            row.activity = previous.activity;
                        }
                    } else if row.modified_at.is_none() {
                        row.modified_at = entry.as_ref().map(|entry| entry.time);
                    }
                    if self.baseline == "last clear"
                        && row.status == Status::Unchanged
                        && entry.is_none()
                        && !self.files.contains_key(&row.path)
                    {
                        continue;
                    }
                    if let Some(old) = self.files.remove(&row.path) {
                        remove_time(&mut self.modification_times, old.modified_at);
                    }
                    if !row.remove_from_tree {
                        if let Some(time) = row.modified_at {
                            *self.modification_times.entry(time).or_default() += 1;
                        }
                        self.files.insert(row.path.clone(), row);
                    }
                    if let Some(mut entry) = entry {
                        if entry.bytes() > self.limit_bytes {
                            let budget = self
                                .limit_bytes
                                .saturating_sub(entry.bytes() - entry.diff.len() + 80);
                            let mut end = budget.min(entry.diff.len());
                            while !entry.diff.is_char_boundary(end) {
                                end -= 1;
                            }
                            entry.diff = format!(
                                "{}\n[entry truncated to history byte limit]",
                                &entry.diff[..end]
                            )
                            .into();
                        }
                        entry.line_count = entry.diff.lines().count();
                        self.bytes += entry.bytes();
                        self.entries.push_back(entry);
                    }
                }
                while self.entries.len() > self.limit_entries || self.bytes > self.limit_bytes {
                    if let Some(entry) = self.entries.pop_front() {
                        self.bytes -= entry.bytes();
                        self.evicted += 1;
                    } else {
                        break;
                    }
                }
                let times = &mut self.modification_times;
                self.files.retain(|_, r| {
                    let keep = r.status != Status::Unchanged || !r.git.trim().is_empty();
                    if !keep {
                        remove_time(times, r.modified_at);
                    }
                    keep
                });
            }
            Update::Progress(message) => {
                self.refreshing = true;
                self.message = message;
            }
            Update::Notice(message) => {
                self.set_notice(message.clone());
                self.message = message;
            }
            Update::WatchStatus(message) => {
                self.set_notice(message.clone());
                self.watch_status = Some(message);
            }
            Update::Done => self.refreshing = false,
        }
    }
}

fn remove_time(times: &mut BTreeMap<SystemTime, usize>, time: Option<SystemTime>) {
    if let Some(time) = time {
        if let Some(count) = times.get_mut(&time) {
            *count -= 1;
            if *count == 0 {
                times.remove(&time);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_expire_and_refreshes_do_not_extend_their_lifetime() {
        let mut model = Model::new(10, 1024);
        model.apply(Update::Notice("Cleared".into()));
        let deadline = model.notice_until.unwrap();
        model.apply(Update::Done);
        assert_eq!(model.notice_until, Some(deadline));
        assert_eq!(
            model.notice_remaining(deadline - NOTICE_LIFETIME),
            Some(1.0)
        );
        assert_eq!(
            model.notice_remaining(deadline - NOTICE_LIFETIME / 2),
            Some(0.5)
        );
        assert!(!model.expire_notice(deadline - Duration::from_millis(1)));
        model.apply(Update::Progress("Refreshing".into()));
        assert_eq!(model.notice_remaining(deadline), None);
        assert!(!model.expire_notice(deadline));
        model.apply(Update::Done);
        assert_eq!(model.notice_until, Some(deadline));
        assert!(model.expire_notice(deadline));
        assert!(model.notice.is_empty());
        assert_eq!(model.notice_remaining(deadline), None);
        assert!(!model.expire_notice(deadline));
    }

    #[test]
    fn dismissing_a_notice_does_not_resurrect_it_or_interrupt_progress() {
        let mut model = Model::new(10, 1024);
        model.apply(Update::Notice("Cleared".into()));
        model.apply(Update::Progress("Refreshing".into()));
        assert!(model.dismiss_notice());
        assert!(model.refreshing);
        assert_eq!(model.message, "Refreshing");
        model.apply(Update::Done);
        assert!(model.notice.is_empty());
        assert_eq!(model.notice_remaining(Instant::now()), None);
        assert!(!model.dismiss_notice());
        model.apply(Update::Notice("New notification".into()));
        assert!(model.notice_remaining(Instant::now()).unwrap() > 0.99);
    }

    #[test]
    fn retention_preserves_live_rows_and_immutable_history() {
        let mut m = Model::new(1, 1024);
        for id in 0..3 {
            m.apply(Update::Files(vec![(
                FileRow {
                    path: format!("{id}"),
                    status: if id == 0 {
                        Status::Modified
                    } else {
                        Status::Unchanged
                    },
                    git: "  ".into(),
                    note: String::new(),
                    activity: Instant::now(),
                    remove_from_tree: false,
                    modified_at: None,
                },
                Some(Entry {
                    id,
                    path: format!("{id}"),
                    time: SystemTime::now(),
                    activity: "edit".into(),
                    diff: "old\nnew".into(),
                    collapsed: false,
                    line_count: 2,
                }),
            )]));
        }
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.evicted, 2);
        assert!(m.files.contains_key("0"));
        assert!(!m.files.contains_key("1"));
        assert_eq!(&*m.entries[0].diff, "old\nnew");
        assert_eq!(m.modification_times.values().sum::<usize>(), m.files.len());
    }
    #[test]
    fn huge_entry_is_bounded_and_utf8_safe() {
        let mut m = Model::new(10, 1024);
        m.apply(Update::Files(vec![(
            FileRow {
                path: "a".into(),
                status: Status::New,
                git: "??".into(),
                note: String::new(),
                activity: Instant::now(),
                remove_from_tree: false,
                modified_at: None,
            },
            Some(Entry {
                id: 1,
                path: "a".into(),
                time: SystemTime::now(),
                activity: "edit".into(),
                diff: "🦀".repeat(5000).into(),
                collapsed: false,
                line_count: 2,
            }),
        )]));
        assert!(m.bytes <= 1024);
        assert_eq!(m.entries.len(), 1);
    }
}
