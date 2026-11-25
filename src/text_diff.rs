//! Read-only, explicit snapshots of two text tabs. No document mutation or IO.
use crate::{Lang, LiteTextApp, Palette};
use eframe::egui;
use std::borrow::Cow;
use std::collections::HashMap;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINES: usize = 200_000;
// A shared work budget bounds quadratic gaps; exhausted gaps become replacements.
const GAP_WORK: usize = 2_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub ignore_whitespace: bool,
    pub ignore_line_endings: bool,
}

#[derive(Default)]
pub(crate) struct State {
    pub open: bool,
    pub left: Option<u64>,
    pub right: Option<u64>,
    pub options: Options,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    current: usize,
    scroll_to: Option<usize>,
    pending: Option<std::sync::mpsc::Receiver<Snapshot>>,
}

impl State {
    /// Closing invalidates the receiver: a detached worker can never publish
    /// into this state or a later window session.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.pending = None;
    }

    fn start(
        &mut self,
        ctx: &egui::Context,
        left_name: &str,
        left: &str,
        right_name: &str,
        right: &str,
    ) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Err("busy");
        }
        // Validate BOTH inputs before cloning either bounded text buffer.
        validate(left)?;
        validate(right)?;
        let (left, right) = (left.to_owned(), right.to_owned());
        let (left_name, right_name) = (left_name.to_owned(), right_name.to_owned());
        let options = self.options;
        let ctx = ctx.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("text-diff".to_owned())
            .spawn(move || {
                if let Ok(diff) = diff(&left, &right, options) {
                    let _ = sender.send(Snapshot {
                        left_name,
                        right_name,
                        options,
                        diff,
                    });
                }
                // Wake the UI even on a disconnected receiver or failed result.
                ctx.request_repaint();
            })
            .map_err(|_| "worker unavailable")?;
        self.pending = Some(receiver);
        self.error = None;
        Ok(())
    }

    fn poll(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let Some(receiver) = &self.pending else {
            return;
        };
        match receiver.try_recv() {
            Ok(snapshot) => {
                self.scroll_to = snapshot.diff.changes.first().copied();
                self.current = 0;
                self.snapshot = Some(snapshot);
                self.pending = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some("Text comparison worker stopped / 文本对比任务已停止".to_owned());
            }
        }
    }
}

struct Snapshot {
    left_name: String,
    right_name: String,
    options: Options,
    diff: Diff,
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    left: Option<usize>,
    right: Option<usize>,
    changed: bool,
}

#[derive(Debug)]
struct Diff {
    left: Vec<String>,
    right: Vec<String>,
    rows: Vec<Row>,
    changes: Vec<usize>,
    coarse: bool,
}

fn validate(text: &str) -> Result<(), &'static str> {
    if text.len() > MAX_BYTES {
        return Err("8 MiB");
    }
    if text.bytes().filter(|&b| b == b'\n').count() >= MAX_LINES {
        return Err("200,000 lines");
    }
    Ok(())
}

fn key(line: &str, options: Options) -> Cow<'_, str> {
    let (body, cr) = line.strip_suffix('\r').map_or((line, false), |s| (s, true));
    let body = if options.ignore_whitespace {
        body.trim()
    } else {
        body
    };
    if cr && !options.ignore_line_endings {
        Cow::Owned(format!("{body}\r"))
    } else {
        Cow::Borrowed(body)
    }
}

// LCS lengths use two linear rows, never an n*m traceback matrix.
fn lengths(a: &[Cow<'_, str>], b: &[Cow<'_, str>], reverse: bool) -> Vec<usize> {
    let mut row = vec![0; b.len() + 1];
    for i in 0..a.len() {
        let mut diagonal = 0;
        for j in 0..b.len() {
            let old = row[j + 1];
            let (ai, bj) = if reverse {
                (a.len() - 1 - i, b.len() - 1 - j)
            } else {
                (i, j)
            };
            row[j + 1] = if a[ai] == b[bj] {
                diagonal + 1
            } else {
                row[j].max(old)
            };
            diagonal = old;
        }
    }
    row
}

fn gap_matches(
    a: &[Cow<'_, str>],
    b: &[Cow<'_, str>],
    x: usize,
    y: usize,
    budget: &mut usize,
    out: &mut Vec<(usize, usize)>,
    coarse: &mut bool,
) {
    let mut prefix = 0;
    while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
        out.push((x + prefix, y + prefix));
        prefix += 1;
    }
    let (a, b, x, y) = (&a[prefix..], &b[prefix..], x + prefix, y + prefix);
    let mut suffix = 0;
    while suffix < a.len().min(b.len()) && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }
    let (aa, bb) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
    if !aa.is_empty() && !bb.is_empty() {
        if aa.len() == 1 {
            if let Some(j) = bb.iter().position(|line| *line == aa[0]) {
                out.push((x, y + j));
            }
        } else if bb.len() == 1 {
            if let Some(i) = aa.iter().position(|line| *line == bb[0]) {
                out.push((x + i, y));
            }
        } else {
            let cost = aa.len().saturating_mul(bb.len());
            if cost > *budget {
                *coarse = true;
            } else {
                *budget -= cost;
                let middle = aa.len() / 2;
                let front = lengths(&aa[..middle], bb, false);
                let back = lengths(&aa[middle..], bb, true);
                let split = (0..=bb.len())
                    .max_by_key(|&j| front[j] + back[bb.len() - j])
                    .unwrap();
                // Release both rows before recursion: linear auxiliary memory.
                drop(front);
                drop(back);
                gap_matches(&aa[..middle], &bb[..split], x, y, budget, out, coarse);
                gap_matches(
                    &aa[middle..],
                    &bb[split..],
                    x + middle,
                    y + split,
                    budget,
                    out,
                    coarse,
                );
            }
        }
    }
    for i in (0..suffix).rev() {
        out.push((x + a.len() - 1 - i, y + b.len() - 1 - i));
    }
}

/// Unique common lines, ordered by an O(n log n) longest increasing subsequence.
fn anchors(a: &[Cow<'_, str>], b: &[Cow<'_, str>]) -> Vec<(usize, usize)> {
    fn positions<'a>(lines: &'a [Cow<'_, str>]) -> HashMap<&'a str, (usize, usize)> {
        let mut map = HashMap::new();
        for (i, line) in lines.iter().enumerate() {
            let entry = map.entry(line.as_ref()).or_insert((i, 0));
            entry.1 += 1;
        }
        map
    }
    let left = positions(a);
    let right = positions(b);
    let pairs: Vec<_> = a
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            let &(_, count) = left.get(line.as_ref())?;
            let &(j, other_count) = right.get(line.as_ref())?;
            (count == 1 && other_count == 1).then_some((i, j))
        })
        .collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; pairs.len()];
    for (i, &(_, j)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&t| pairs[t].1 < j);
        if at > 0 {
            previous[i] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut result = Vec::new();
    let mut cursor = tails.last().copied();
    while let Some(i) = cursor {
        result.push(pairs[i]);
        cursor = previous[i];
    }
    result.reverse();
    result
}

fn diff(left: &str, right: &str, options: Options) -> Result<Diff, &'static str> {
    validate(left)?;
    validate(right)?;
    let a: Vec<_> = left.split('\n').map(|s| key(s, options)).collect();
    let b: Vec<_> = right.split('\n').map(|s| key(s, options)).collect();
    let mut matches = Vec::new();
    let mut budget = GAP_WORK;
    let mut coarse = false;
    let (mut x, mut y) = (0, 0);
    for (i, j) in anchors(&a, &b)
        .into_iter()
        .chain(std::iter::once((a.len(), b.len())))
    {
        gap_matches(
            &a[x..i],
            &b[y..j],
            x,
            y,
            &mut budget,
            &mut matches,
            &mut coarse,
        );
        if i < a.len() {
            matches.push((i, j));
        }
        x = i + 1;
        y = j + 1;
    }
    let mut rows = Vec::new();
    let mut changes = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (i, j) in matches
        .into_iter()
        .chain(std::iter::once((a.len(), b.len())))
    {
        if x < i || y < j {
            changes.push(rows.len());
            for k in 0..(i - x).max(j - y) {
                rows.push(Row {
                    left: (x + k < i).then_some(x + k),
                    right: (y + k < j).then_some(y + k),
                    changed: true,
                });
            }
        }
        if i < a.len() {
            rows.push(Row {
                left: Some(i),
                right: Some(j),
                changed: false,
            });
        }
        x = i + 1;
        y = j + 1;
    }
    Ok(Diff {
        left: left.split('\n').map(str::to_owned).collect(),
        right: right.split('\n').map(str::to_owned).collect(),
        rows,
        changes,
        coarse,
    })
}

fn label(lang: Lang, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::ZhCn => zh,
        Lang::En => en,
    }
}

impl LiteTextApp {
    pub fn text_diff_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.text_diff.open {
            self.text_diff.close();
            return;
        }
        let lang = self.lang;
        let state = &mut self.text_diff;
        state.poll();
        let mut open = state.open;
        egui::Window::new(label(lang, "文本对比（只读快照）", "Text comparison (read-only snapshots)"))
            .id(egui::Id::new("text_diff_window"))
            .open(&mut open).default_size(egui::vec2(1000.0, 640.0)).show(ctx, |ui| {
                ui.memory_mut(|memory| memory.set_modal_layer(ui.layer_id()));
                ui.label(label(lang, "点击比较时捕获当前文本（含未保存更改），之后编辑或关闭标签不会改变快照。不会修改原文件。", "Compare captures current text, including unsaved edits. Editing or closing tabs does not change the snapshot. Source files are never modified."));
                ui.small(label(lang, "每侧最多 8 MiB / 200,000 行，超限不复制、不比较，以限制内存和界面等待；仅文本标签可选。", "Text tabs only; each side is limited to 8 MiB / 200,000 lines before copying to bound memory and UI work."));
                ui.horizontal_wrapped(|ui| {
                    for (id, selected, title) in [("diff_left", &mut state.left, label(lang, "左侧", "Left")), ("diff_right", &mut state.right, label(lang, "右侧", "Right"))] {
                        ui.label(title);
                        let name = self.files.iter().find(|f| Some(f.id) == *selected && f.draft_text.is_some()).map(|f| f.name.as_str()).unwrap_or(label(lang, "选择文本标签", "Select text tab"));
                        egui::ComboBox::from_id_salt(id).selected_text(name).width(180.0).show_ui(ui, |ui| {
                            for file in self.files.iter().filter(|f| f.draft_text.is_some()) {
                                ui.selectable_value(selected, Some(file.id), &file.name);
                            }
                        });
                    }
                    if ui.add_enabled(state.pending.is_none(), egui::Button::new(label(lang, "比较 / 刷新快照", "Compare / refresh snapshot"))).clicked() {
                        let left = self.files.iter().find(|f| Some(f.id) == state.left && f.draft_text.is_some());
                        let right = self.files.iter().find(|f| Some(f.id) == state.right && f.draft_text.is_some());
                        match (left, right) {
                            (Some(left), Some(right)) => {
                                if let Err(limit) = state.start(ctx, &left.name, left.draft_text.as_deref().unwrap(), &right.name, right.draft_text.as_deref().unwrap()) {
                                    state.error = Some(format!("{} {limit}", label(lang, "无法开始比较（单侧限制 / 任务状态）：", "Cannot compare (per-side limit / worker state):")));
                                }
                            },
                            _ => state.error = Some(label(lang, "请选择两个仍然打开的文本标签。", "Select two currently open text tabs.").to_owned()),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut state.options.ignore_whitespace, label(lang, "忽略行首尾空白", "Ignore leading/trailing whitespace"));
                    ui.checkbox(&mut state.options.ignore_line_endings, label(lang, "忽略 CRLF / LF 差异", "Ignore CRLF / LF differences"));
                });
                if state.pending.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(label(lang, "后台比较中；关闭窗口将丢弃本次结果。", "Comparing in background; closing discards this result."));
                    });
                }
                if let Some(error) = &state.error { ui.colored_label(pal.danger, error); }
                let Some(snapshot) = &state.snapshot else { return; };
                if snapshot.options != state.options {
                    ui.colored_label(pal.recovery_pending, label(lang, "选项已改变，请重新比较。下方仍是旧快照。", "Options changed; compare again. The previous snapshot is still shown."));
                }
                ui.separator();
                ui.label(format!("{}  ↔  {}", snapshot.left_name, snapshot.right_name));
                let count = snapshot.diff.changes.len();
                ui.horizontal(|ui| {
                    if ui.add_enabled(count > 0, egui::Button::new(label(lang, "上一处差异", "Previous change"))).clicked() {
                        state.current = (state.current + count - 1) % count;
                        state.scroll_to = Some(snapshot.diff.changes[state.current]);
                    }
                    if ui.add_enabled(count > 0, egui::Button::new(label(lang, "下一处差异", "Next change"))).clicked() {
                        state.current = (state.current + 1) % count;
                        state.scroll_to = Some(snapshot.diff.changes[state.current]);
                    }
                    ui.label(if count == 0 { label(lang, "文本相同", "Texts are equal").to_owned() } else { format!("{} / {}", state.current + 1, count) });
                });
                if snapshot.diff.coarse {
                    ui.colored_label(pal.recovery_pending, label(lang, "复杂差异已达到计算预算：部分区段按整体替换显示（并非最小差分）。", "Diff budget reached: some gaps are shown as whole replacements, not a minimal diff."));
                }
                ui.small(label(lang, "红色：左侧删除；绿色：右侧新增。CRLF/LF 差异也会高亮。长行显示前 2,048 字符（快照与比较仍完整）。", "Red: removed on left; green: added on right. CRLF/LF changes are highlighted too. Long lines display the first 2,048 characters; comparison remains complete."));
                let height = ui.text_style_height(&egui::TextStyle::Monospace).max(16.0) + 4.0;
                // show_rows includes item spacing in its virtual stride; keep it
                // identical to the navigation offset and the painted row height.
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut area = egui::ScrollArea::both().id_salt("text_diff_rows").auto_shrink([false, false]);
                if let Some(row) = state.scroll_to.take() { area = area.vertical_scroll_offset(row as f32 * height); }
                area.show_rows(ui, height, snapshot.diff.rows.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let width = ui.available_width().max(900.0);
                    for index in range {
                        let row = &snapshot.diff.rows[index];
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                        for (side, number, lines, color) in [(0, row.left, &snapshot.diff.left, pal.danger), (1, row.right, &snapshot.diff.right, pal.recovery_backed_up)] {
                            let cell = egui::Rect::from_min_size(rect.min + egui::vec2(side as f32 * width / 2.0, 0.0), egui::vec2(width / 2.0 - 2.0, height));
                            let painter = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
                            painter.rect_filled(cell, 0.0, if row.changed && number.is_some() { color.gamma_multiply(0.18) } else { pal.bg_inset });
                            if let Some(number) = number {
                                painter.text(cell.min + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, format!("{:>6}", number + 1), egui::FontId::monospace(12.0), pal.text_faint);
                                let text: String = lines[number].trim_end_matches('\r').chars().take(2048).collect();
                                painter.text(cell.min + egui::vec2(60.0, 2.0), egui::Align2::LEFT_TOP, text, egui::FontId::monospace(13.0), pal.text_main);
                            }
                        }
                    }
                });
            });
        if open {
            state.open = true;
        } else {
            state.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compare(a: &str, b: &str) -> Diff {
        diff(a, b, Options::default()).unwrap()
    }
    fn check_coverage(d: &Diff) {
        assert_eq!(
            d.rows.iter().filter_map(|r| r.left).collect::<Vec<_>>(),
            (0..d.left.len()).collect::<Vec<_>>()
        );
        assert_eq!(
            d.rows.iter().filter_map(|r| r.right).collect::<Vec<_>>(),
            (0..d.right.len()).collect::<Vec<_>>()
        );
        for row in &d.rows {
            if !row.changed {
                assert_eq!(d.left[row.left.unwrap()], d.right[row.right.unwrap()]);
            }
        }
    }
    #[test]
    fn equal_and_empty() {
        for text in ["", "a", "a\nb\n", "\n\n"] {
            let d = compare(text, text);
            assert!(d.changes.is_empty());
            check_coverage(&d);
        }
    }
    #[test]
    fn insert_delete_replace() {
        for (a, b) in [
            ("a\nc", "a\nb\nc"),
            ("a\nb\nc", "a\nc"),
            ("a\nb\nc", "a\nx\nc"),
            ("a", "a\n"),
            ("", "text"),
        ] {
            let d = compare(a, b);
            assert_eq!(d.changes.len(), 1);
            check_coverage(&d);
        }
        let d = compare("a\nb\nc", "a\nx\nc");
        assert_eq!(
            d.rows[1],
            Row {
                left: Some(1),
                right: Some(1),
                changed: true
            }
        );
    }
    #[test]
    fn unicode_and_crlf() {
        let d = compare("你好\n🦀\n終", "你好\n🌏\n終");
        check_coverage(&d);
        assert_eq!(d.changes, vec![1]);
        assert_eq!(compare("a\r\nb\r\n", "a\nb\n").changes.len(), 1);
        assert!(
            diff(
                "a\r\nb\r\n",
                "a\nb\n",
                Options {
                    ignore_line_endings: true,
                    ..Options::default()
                }
            )
            .unwrap()
            .changes
            .is_empty()
        );
        assert!(!compare("a\r", "a").changes.is_empty());
    }
    #[test]
    fn whitespace_options_do_not_change_input() {
        let a = "  α\t\n\t\nfoo bar";
        let b = "α\n\nfoo bar";
        assert!(!compare(a, b).changes.is_empty());
        let d = diff(
            a,
            b,
            Options {
                ignore_whitespace: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert!(d.changes.is_empty());
        assert_eq!(d.left[0], "  α\t");
        assert!(
            !diff(
                "foo bar",
                "foobar",
                Options {
                    ignore_whitespace: true,
                    ..Options::default()
                }
            )
            .unwrap()
            .changes
            .is_empty()
        );
    }
    #[test]
    fn large_boundaries_and_budget() {
        assert!(validate(&"x".repeat(MAX_BYTES)).is_ok());
        assert!(validate(&"x".repeat(MAX_BYTES + 1)).is_err());
        let lines = "\n".repeat(MAX_LINES - 1);
        let d = compare(&lines, &lines);
        assert_eq!(d.rows.len(), MAX_LINES);
        assert!(d.changes.is_empty());
        assert!(diff(&(lines + "\n"), "", Options::default()).is_err());
        let a = "a\nb\n".repeat(10_000);
        let b = "c\nd\n".repeat(10_000);
        let d = compare(&a, &b);
        assert!(d.coarse);
        check_coverage(&d);
    }
    #[test]
    fn window_renders_snapshots_in_both_languages_without_touching_documents() {
        for lang in [Lang::ZhCn, Lang::En] {
            let mut app = LiteTextApp {
                lang,
                ..Default::default()
            };
            app.new_document();
            let original = app.files[0].draft_text.clone();
            let revision = app.files[0].content_revision;
            app.text_diff.open = true;
            app.text_diff.snapshot = Some(Snapshot {
                left_name: "left".to_owned(),
                right_name: "right".to_owned(),
                options: Options::default(),
                diff: compare("one\nold\nlast", "one\nnew\nlast"),
            });
            app.text_diff.scroll_to = Some(1);
            let ctx = egui::Context::default();
            for pal in [&crate::DARK, &crate::LIGHT] {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.text_diff_window(ctx, pal),
                );
                assert!(!output.shapes.is_empty());
                assert_eq!(app.files[0].draft_text, original);
                assert_eq!(app.files[0].content_revision, revision);
                assert_eq!(app.text_diff.snapshot.as_ref().unwrap().diff.left[1], "old");
            }
        }
    }
    #[test]
    fn async_worker_captures_inputs_options_and_refuses_duplicate_refresh() {
        let ctx = egui::Context::default();
        let mut state = State {
            open: true,
            ..Default::default()
        };
        state
            .start(&ctx, "left", "a\nold", "right", "a\nnew")
            .unwrap();
        assert!(state.pending.is_some());
        assert_eq!(state.start(&ctx, "other", "", "other", ""), Err("busy"));
        state.options.ignore_whitespace = true;
        // Block only the test, never the rendering thread. The real worker must
        // deliver its immutable captured options and original input text.
        let snapshot = state
            .pending
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(snapshot.left_name, "left");
        assert_eq!(snapshot.right_name, "right");
        assert!(!snapshot.options.ignore_whitespace);
        assert_eq!(snapshot.diff.left[1], "old");
        assert_eq!(snapshot.diff.right[1], "new");
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(snapshot)
            .unwrap_or_else(|_| panic!("receiver disconnected"));
        state.pending = Some(rx);
        state.poll();
        assert!(state.pending.is_none());
        assert_eq!(state.snapshot.as_ref().unwrap().diff.changes, vec![1]);
        assert_eq!(state.scroll_to, Some(1));
    }

    #[test]
    fn close_invalidates_old_session_and_limits_fail_before_starting_worker() {
        let ctx = egui::Context::default();
        let mut state = State {
            open: true,
            ..Default::default()
        };
        assert_eq!(
            state.start(&ctx, "left", "ok", "right", &"x".repeat(MAX_BYTES + 1)),
            Err("8 MiB")
        );
        assert!(state.pending.is_none());
        let (old_sender, old_receiver) = std::sync::mpsc::channel();
        state.pending = Some(old_receiver);
        state.close();
        assert!(!state.open);
        assert!(state.pending.is_none());
        state.open = true;
        state
            .start(&ctx, "new-left", "new", "new-right", "new")
            .unwrap();
        // An older session cannot replace a later session's snapshot.
        assert!(
            old_sender
                .send(Snapshot {
                    left_name: "stale".to_owned(),
                    right_name: "stale".to_owned(),
                    options: Options::default(),
                    diff: compare("stale", "stale"),
                })
                .is_err()
        );
        let snapshot = state
            .pending
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(snapshot.left_name, "new-left");
        assert_eq!(snapshot.diff.left, vec!["new"]);
        // External callers may set open=false; the window entry cancels too.
        let mut app = LiteTextApp::default();
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.text_diff.pending = Some(receiver);
        app.text_diff_window(&ctx, &crate::DARK);
        assert!(app.text_diff.pending.is_none());
    }

    #[test]
    fn repeated_gaps_and_reordered_anchors_preserve_all_lines() {
        for a in ["a\nb\na\nc", "x\nx\nx", "c\nb\na", "a\n\n\nb"] {
            for b in ["b\na\nb\nc", "x\nx", "a\nb\nc", "a\n\nb"] {
                check_coverage(&compare(a, b));
            }
        }
    }
}
