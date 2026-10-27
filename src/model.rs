use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Instant, SystemTime},
};

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
    Reset { baseline: String, reason: String },
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
    pub files: BTreeMap<String, FileRow>,
    pub entries: VecDeque<Entry>,
    pub bytes: usize,
    pub evicted: usize,
    pub baseline: String,
    pub message: String,
    pub refreshing: bool,
    pub notice: String,
    pub watch_status: Option<String>,
    limit_entries: usize,
    limit_bytes: usize,
}
impl Model {
    pub fn new(entries: usize, bytes: usize) -> Self {
        Self {
            files: BTreeMap::new(),
            entries: VecDeque::new(),
            bytes: 0,
            evicted: 0,
            baseline: "HEAD".into(),
            message: "Reading repository…".into(),
            refreshing: true,
            notice: String::new(),
            watch_status: None,
            limit_entries: entries,
            limit_bytes: bytes,
        }
    }
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Reset { baseline, reason } => {
                self.files.clear();
                self.entries.clear();
                self.bytes = 0;
                self.evicted = 0;
                self.baseline = baseline;
                self.notice = reason.clone();
                self.message = reason;
                self.refreshing = true;
            }
            Update::Files(rows) => {
                for (row, entry) in rows {
                    if self.baseline == "last clear"
                        && row.status == Status::Unchanged
                        && entry.is_none()
                        && !self.files.contains_key(&row.path)
                    {
                        continue;
                    }
                    self.files.insert(row.path.clone(), row);
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
                let retained: std::collections::HashSet<_> =
                    self.entries.iter().map(|e| e.path.as_str()).collect();
                self.files.retain(|p, r| {
                    r.status != Status::Unchanged || r.git != "  " || retained.contains(p.as_str())
                });
            }
            Update::Progress(message) => {
                self.refreshing = true;
                self.message = message;
            }
            Update::Notice(message) => {
                self.notice = message.clone();
                self.message = message;
            }
            Update::WatchStatus(message) => self.watch_status = Some(message),
            Update::Done => self.refreshing = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
