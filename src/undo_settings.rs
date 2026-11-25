//! Configure egui's private undoer through its public snapshot API. Rebuild
//! only when the limit changes, retaining both undo and redo branches.
use eframe::egui;
pub(crate) const DEFAULT: usize = 50;
pub(crate) const MAX: usize = 500;
pub(crate) fn limit(ctx: &egui::Context) -> usize {
    ctx.data(|d| d.get_temp::<usize>(egui::Id::new("workspace_undo_limit"))).unwrap_or(DEFAULT).clamp(20, MAX)
}
pub(crate) fn configure(ctx: &egui::Context, id: egui::Id, text: &str) {
    let limit = limit(ctx);
    let key = id.with("configured_undo_limit");
    if ctx.data(|d| d.get_temp::<usize>(key)) == Some(limit) { return; }
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let current = (state.cursor.char_range().unwrap_or_default(), text.to_owned());
    let old = state.undoer();
    let mut walk = old.clone();
    let mut cursor = current.clone();
    let mut past = Vec::new();
    for _ in 0..limit {
        let Some(previous) = walk.undo(&cursor).cloned() else { break; };
        cursor = previous.clone(); past.push(previous);
    }
    let mut walk = old;
    let mut cursor = current.clone();
    let mut future = Vec::new();
    for _ in 0..limit {
        let Some(next) = walk.redo(&cursor).cloned() else { break; };
        cursor = next.clone(); future.push(next);
    }
    past.truncate(limit.saturating_sub(future.len()));
    let mut rebuilt = egui::util::undoer::Undoer::with_settings(egui::util::undoer::Settings {
        // egui counts the current state among its saved points.
        max_undos: limit + 1, ..Default::default()
    });
    for point in past.into_iter().rev() { rebuilt.add_undo(&point); }
    rebuilt.add_undo(&current);
    for point in &future { rebuilt.add_undo(point); }
    let mut cursor = future.last().cloned().unwrap_or(current);
    for _ in 0..future.len() { cursor = rebuilt.undo(&cursor).cloned().unwrap(); }
    // The combined undo/redo branch remains within the configured budget.
    state.set_undoer(rebuilt); state.store(ctx, id);
    ctx.data_mut(|d| d.insert_temp(key, limit));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_history_respects_configured_cap_and_retains_redo() {
        let ctx = egui::Context::default(); let id = egui::Id::new("limit-test");
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("workspace_undo_limit"), 20usize));
        configure(&ctx, id, "0");
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let mut history = state.undoer();
        let cursor = egui::text::CCursorRange::default();
        for n in 0..30 { history.add_undo(&(cursor, n.to_string())); }
        let mut current = (cursor, "29".to_owned()); let mut count = 0;
        while let Some(previous) = history.undo(&current).cloned() { current = previous; count += 1; }
        assert_eq!(count, 20);
        state.set_undoer(history); state.store(&ctx, id);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("workspace_undo_limit"), 50usize));
        configure(&ctx, id, &current.1);
        let mut history = egui::TextEdit::load_state(&ctx, id).unwrap().undoer();
        let mut redos = 0;
        while let Some(next) = history.redo(&current).cloned() { current = next; redos += 1; }
        assert_eq!(redos, 20); assert_eq!(current.1, "29");
    }
}
