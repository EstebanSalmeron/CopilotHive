//! Directory browser modal (`o`) to explore folders/files and open a directory in a new tab.
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered, hints, trunc};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

fn next_req() -> u64 {
    NEXT_REQ.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
}

pub(super) struct Loaded {
    pub(super) req: u64,
    pub(super) entries: Result<Vec<Entry>, String>,
}

pub(super) enum Action {
    None,
    Close,
    /// Load directory entries in a blocking task: `(req_id, dir, show_hidden)`.
    Load(u64, PathBuf, bool),
    /// Open the selected directory in a new tab.
    Open(PathBuf),
}

/// Reads `dir` and returns `..` (when a parent exists), then subdirectories and files
/// sorted alphabetically case-insensitive.
pub(super) fn read_dir(dir: &Path, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type().ok();
        let is_dir =
            ft.is_some_and(|t| t.is_dir()) || (ft.is_none_or(|t| t.is_symlink()) && path.is_dir());
        items.push(Entry { name, path, is_dir });
    }
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut out = Vec::with_capacity(items.len() + 1);
    if let Some(parent) = dir.parent() {
        out.push(Entry {
            name: "..".into(),
            path: parent.to_path_buf(),
            is_dir: true,
        });
    }
    out.extend(items);
    Ok(out)
}

pub(super) struct Browser {
    dir: PathBuf,
    entries: Vec<Entry>,
    sel: usize,
    show_hidden: bool,
    loading: Option<u64>,
    error: Option<String>,
    /// Folder name to select automatically after loading the parent directory.
    pending_select: Option<String>,
}

impl Browser {
    pub(super) fn open(dir: PathBuf) -> (Self, u64) {
        let req = next_req();
        (
            Browser {
                dir,
                entries: vec![],
                sel: 0,
                show_hidden: false,
                loading: Some(req),
                error: None,
                pending_select: None,
            },
            req,
        )
    }

    pub(super) fn on_loaded(&mut self, ev: Loaded) {
        if self.loading != Some(ev.req) {
            return;
        }
        self.loading = None;
        match ev.entries {
            Ok(entries) => {
                self.error = None;
                self.entries = entries;
                self.sel = self
                    .pending_select
                    .take()
                    .and_then(|name| self.entries.iter().position(|e| e.name == name))
                    .unwrap_or(0);
            }
            Err(e) => {
                self.entries.clear();
                self.sel = 0;
                self.pending_select = None;
                self.error = Some(e);
            }
        }
    }

    fn navigate(&mut self, dir: PathBuf, pending_select: Option<String>) -> Action {
        let req = next_req();
        self.dir = dir.clone();
        self.entries.clear();
        self.sel = 0;
        self.loading = Some(req);
        self.error = None;
        self.pending_select = pending_select;
        Action::Load(req, dir, self.show_hidden)
    }

    fn go_parent(&mut self) -> Action {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return Action::None;
        };
        let prev = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.navigate(parent, prev)
    }

    pub(super) fn on_key(&mut self, code: KeyCode) -> Action {
        let last = self.entries.len().saturating_sub(1);
        match code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Char('o') => return Action::Open(self.dir.clone()),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(e) = self.entries.get(self.sel) {
                    if e.name == ".." {
                        return self.go_parent();
                    }
                    if e.is_dir {
                        return self.navigate(e.path.clone(), None);
                    }
                }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                return self.go_parent();
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                let dir = self.dir.clone();
                return self.navigate(dir, None);
            }
            KeyCode::Char('~') => {
                if let Some(home) = std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(PathBuf::from)
                {
                    return self.navigate(home, None);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect) {
        let w = area.width.saturating_sub(6).min(80);
        let h = area.height.saturating_sub(4).min(26);
        let r = centered(area, w, h);
        f.render_widget(Clear, r);
        let [body, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(r);

        let path = self.dir.to_string_lossy();
        let title_w = body.width.saturating_sub(13) as usize;
        let title = Line::styled(
            tr_fmt!(" Abrir · {} ", " Open · {} ", trunc(&path, title_w)),
            bold(c_accent()),
        );
        let mut blk = block(title, c_accent()).padding(Padding::horizontal(1));
        if self.show_hidden {
            blk = blk.title_bottom(
                Line::styled(tr(" ocultos: sí ", " hidden: on "), fg(c_dim())).right_aligned(),
            );
        }

        let inner = blk.inner(body);
        let rows = inner.height as usize;
        let top = (self.sel + 1).saturating_sub(rows);
        let lw = inner.width as usize;

        let lines: Vec<Line<'static>> = if self.loading.is_some() {
            vec![Line::styled(tr("Cargando…", "Loading…"), fg(c_running()))]
        } else if let Some(e) = &self.error {
            vec![Line::styled(format!("✖ {e}"), fg(c_error()))]
        } else if self.entries.is_empty() {
            vec![Line::styled(
                tr("(carpeta vacía)", "(empty folder)"),
                fg(c_dim()),
            )]
        } else {
            self.entries
                .iter()
                .enumerate()
                .skip(top)
                .take(rows)
                .map(|(i, e)| {
                    let label = if e.name == ".." {
                        "..".to_string()
                    } else if e.is_dir {
                        format!("{}/", e.name)
                    } else {
                        e.name.clone()
                    };
                    let text = trunc(&label, lw.saturating_sub(2));
                    if i == self.sel {
                        Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                    } else if e.is_dir {
                        Line::styled(format!("  {text}"), fg(c_fg()))
                    } else {
                        Line::styled(format!("  {text}"), fg(c_dim()))
                    }
                })
                .collect()
        };
        f.render_widget(Paragraph::new(lines).block(blk), body);

        let h = hints(&[
            ("o", tr("abrir", "open")),
            ("Enter/⌫", tr("nav", "nav")),
            (".", tr("ocultos", "hidden")),
            ("~", "home"),
            ("Esc", tr("cerrar", "close")),
        ]);
        f.render_widget(Paragraph::new(h).style(Style::new().bg(c_bg())), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("copilot-hive-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_dir_orders_parent_dirs_files_and_filters_hidden() {
        let root = temp_dir("listing");
        std::fs::create_dir(root.join("Beta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("Zebra.txt"), "").unwrap();
        std::fs::write(root.join("apple.txt"), "").unwrap();
        std::fs::write(root.join(".env"), "").unwrap();

        let visible = read_dir(&root, false).unwrap();
        let names: Vec<_> = visible.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "alpha", "Beta", "apple.txt", "Zebra.txt"]);
        assert!(visible[0].is_dir && visible[1].is_dir && visible[2].is_dir);
        assert!(!visible[3].is_dir && !visible[4].is_dir);

        let all = read_dir(&root, true).unwrap();
        let names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "..",
                ".git",
                "alpha",
                "Beta",
                ".env",
                "apple.txt",
                "Zebra.txt"
            ]
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn navigation_parent_remember_open_file_ignore_and_stale_discard() {
        let root = temp_dir("nav");
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(root.join("readme.md"), "").unwrap();
        std::fs::write(root.join(".hidden"), "").unwrap();

        let (mut b, req0) = Browser::open(root.clone());
        // Stale response is ignored.
        b.on_loaded(Loaded {
            req: req0 + 99,
            entries: Ok(vec![]),
        });
        assert!(b.loading == Some(req0));

        // Deliver initial directory listing.
        b.on_loaded(Loaded {
            req: req0,
            entries: read_dir(&root, false),
        });
        assert!(b.loading.is_none());
        let names: Vec<_> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "sub", "readme.md"]);

        // Pressing Enter on a file does nothing.
        b.on_key(KeyCode::End);
        assert_eq!(b.entries[b.sel].name, "readme.md");
        assert!(matches!(b.on_key(KeyCode::Enter), Action::None));

        // Toggle hidden files reloads with show_hidden = true.
        let Action::Load(req_h, dir_h, hidden) = b.on_key(KeyCode::Char('.')) else {
            panic!("expected Action::Load on toggle hidden");
        };
        assert_eq!((dir_h, hidden), (root.clone(), true));
        b.on_loaded(Loaded {
            req: req_h,
            entries: read_dir(&root, true),
        });
        assert!(b.entries.iter().any(|e| e.name == ".hidden"));

        // Enter subdirectory "sub".
        let sub_idx = b.entries.iter().position(|e| e.name == "sub").unwrap();
        b.sel = sub_idx;
        let Action::Load(req_sub, dir_sub, _) = b.on_key(KeyCode::Enter) else {
            panic!("expected Action::Load for sub");
        };
        assert_eq!(dir_sub, sub);
        b.on_loaded(Loaded {
            req: req_sub,
            entries: read_dir(&sub, true),
        });

        // Open current directory returns Action::Open(sub).
        assert!(matches!(b.on_key(KeyCode::Char('o')), Action::Open(p) if p == sub));

        // Backspace goes to parent and remembers "sub" as selected.
        let Action::Load(req_up, dir_up, _) = b.on_key(KeyCode::Backspace) else {
            panic!("expected Action::Load for parent");
        };
        assert_eq!(dir_up, root);
        // Stale load from sub is discarded.
        b.on_loaded(Loaded {
            req: req_sub,
            entries: Ok(vec![]),
        });
        assert!(b.loading == Some(req_up));
        b.on_loaded(Loaded {
            req: req_up,
            entries: read_dir(&root, true),
        });
        assert_eq!(b.entries[b.sel].name, "sub");

        assert!(matches!(b.on_key(KeyCode::Esc), Action::Close));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn root_symlinks_and_error_recovery() {
        // Root directory has no ".." entry and Backspace is a no-op.
        let root_entries = read_dir(Path::new("/"), false).unwrap();
        assert!(!root_entries.iter().any(|e| e.name == ".."));
        let (mut rb, rreq) = Browser::open(PathBuf::from("/"));
        rb.on_loaded(Loaded {
            req: rreq,
            entries: Ok(root_entries),
        });
        assert!(matches!(rb.on_key(KeyCode::Backspace), Action::None));

        // Symlinks: valid dir symlink is navigable; broken symlink is treated as non-dir.
        let dir = temp_dir("symlinks");
        let target = dir.join("real_dir");
        std::fs::create_dir(&target).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&target, dir.join("link_dir")).unwrap();
            symlink(dir.join("missing_target"), dir.join("broken_link")).unwrap();

            let entries = read_dir(&dir, false).unwrap();
            let link_dir = entries.iter().find(|e| e.name == "link_dir").unwrap();
            let broken = entries.iter().find(|e| e.name == "broken_link").unwrap();
            assert!(link_dir.is_dir);
            assert!(!broken.is_dir);
        }

        // Error on unreadable/missing directory is shown and Backspace recovers to parent.
        let missing = dir.join("does_not_exist");
        let (mut eb, req) = Browser::open(missing.clone());
        eb.on_loaded(Loaded {
            req,
            entries: read_dir(&missing, false),
        });
        assert!(eb.error.is_some());
        assert!(eb.entries.is_empty());
        let Action::Load(req_up, parent, _) = eb.on_key(KeyCode::Backspace) else {
            panic!("expected Backspace to recover to parent");
        };
        assert_eq!(parent, dir);
        eb.on_loaded(Loaded {
            req: req_up,
            entries: read_dir(&dir, false),
        });
        assert!(eb.error.is_none());
        assert!(!eb.entries.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_list_paging_utf8_and_small_terminal_render() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = PathBuf::from("/tmp/proyecto_日本語_🚀");
        let (mut b, req) = Browser::open(dir.clone());
        let mut entries: Vec<Entry> = (0..30)
            .map(|i| Entry {
                name: format!("dir_{i:02}_日本語_carpeta_muy_larga"),
                path: dir.join(format!("dir_{i:02}")),
                is_dir: true,
            })
            .collect();
        entries.insert(
            0,
            Entry {
                name: "..".into(),
                path: PathBuf::from("/tmp"),
                is_dir: true,
            },
        );
        b.on_loaded(Loaded {
            req,
            entries: Ok(entries),
        });

        b.on_key(KeyCode::PageDown);
        assert_eq!(b.sel, 10);
        b.on_key(KeyCode::PageUp);
        assert_eq!(b.sel, 0);
        b.on_key(KeyCode::End);
        assert_eq!(b.sel, 30);

        let render = |b: &Browser, w: u16, h: u16| -> String {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| b.draw(f, f.area())).unwrap();
            let buf = term.backend().buffer();
            (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        // At End, scrolled view shows the last item with selection marker.
        let s = render(&b, 60, 18);
        assert!(s.contains("▶ dir_29_"), "missing scrolled item:\n{s}");

        b.on_key(KeyCode::Home);
        assert_eq!(b.sel, 0);
        let s_top = render(&b, 60, 18);
        assert!(s_top.contains("▶ .."), "missing top item:\n{s_top}");

        // Tiny terminal does not panic.
        let _ = render(&b, 20, 6);
    }
}
