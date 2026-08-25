use serviceos_userspace_runtime::DesktopAppId;

use crate::{APP_COUNT, DesktopState, windows::visible_on_workspace};

#[derive(Clone, Copy)]
pub(crate) struct SwitcherModel {
    pub(crate) candidates: [DesktopAppId; APP_COUNT],
    pub(crate) count: usize,
}

impl SwitcherModel {
    pub(crate) const EMPTY: Self = Self {
        candidates: [DesktopAppId::Settings; APP_COUNT],
        count: 0,
    };

    pub(crate) fn target(&self, selection: usize) -> Option<DesktopAppId> {
        if self.count == 0 {
            return None;
        }
        Some(self.candidates[selection % self.count])
    }
}

pub(crate) fn switcher_model(state: &DesktopState) -> SwitcherModel {
    let mut model = SwitcherModel::EMPTY;
    for app_id in state.recent_focus[..state.recent_focus_len].iter().copied() {
        if !visible_on_workspace(state, app_id) {
            continue;
        }
        if model.count == APP_COUNT {
            break;
        }
        model.candidates[model.count] = app_id;
        model.count += 1;
    }
    model
}

pub(crate) fn advance_selection(count: usize, selection: usize, forward: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let selection = selection % count;
    if forward {
        (selection + 1) % count
    } else {
        (selection + count - 1) % count
    }
}

pub(crate) fn open_selection(model: &SwitcherModel, focused: Option<DesktopAppId>) -> usize {
    if model.count <= 1 {
        return 0;
    }
    match focused.and_then(|app_id| {
        model.candidates[..model.count]
            .iter()
            .position(|candidate| *candidate == app_id)
    }) {
        Some(index) => advance_selection(model.count, index, true),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::mru_promote;

    fn assert_mru(recent: &[DesktopAppId], expected: &[DesktopAppId]) {
        assert_eq!(recent.len(), expected.len());
        for (index, app_id) in expected.iter().enumerate() {
            assert_eq!(recent[index], *app_id, "slot {index}");
        }
    }

    #[test]
    fn mru_promote_moves_existing_app_to_front() {
        let mut recent = [DesktopAppId::Settings; APP_COUNT];
        let mut len = 0usize;
        for app_id in [
            DesktopAppId::Settings,
            DesktopAppId::Files,
            DesktopAppId::Media,
            DesktopAppId::Monitor,
            DesktopAppId::Terminal,
            DesktopAppId::SoftwareCenter,
        ] {
            mru_promote(&mut recent, &mut len, app_id);
        }
        assert_eq!(len, APP_COUNT);
        mru_promote(&mut recent, &mut len, DesktopAppId::Settings);
        assert_eq!(recent[0], DesktopAppId::Settings);
        assert_mru(
            &recent,
            &[
                DesktopAppId::Settings,
                DesktopAppId::SoftwareCenter,
                DesktopAppId::Terminal,
                DesktopAppId::Monitor,
                DesktopAppId::Media,
                DesktopAppId::Files,
            ],
        );
    }

    #[test]
    fn mru_promote_caps_at_buffer_len() {
        let mut recent = [DesktopAppId::Settings; 3];
        let mut len = 0usize;
        for app_id in [
            DesktopAppId::Files,
            DesktopAppId::Monitor,
            DesktopAppId::Terminal,
            DesktopAppId::SoftwareCenter,
        ] {
            mru_promote(&mut recent, &mut len, app_id);
        }
        assert_eq!(len, 3);
        assert_mru(
            &recent,
            &[
                DesktopAppId::SoftwareCenter,
                DesktopAppId::Terminal,
                DesktopAppId::Monitor,
            ],
        );
    }

    #[test]
    fn advance_wraps_forward_and_backward() {
        assert_eq!(advance_selection(5, 4, true), 0);
        assert_eq!(advance_selection(5, 0, false), 4);
        assert_eq!(advance_selection(1, 0, true), 0);
        assert_eq!(advance_selection(1, 7, false), 0);
        assert_eq!(advance_selection(0, 3, true), 0);
        assert_eq!(advance_selection(5, 9, true), 0);
    }

    #[test]
    fn open_selection_skips_focused_app() {
        let model = SwitcherModel {
            candidates: [
                DesktopAppId::Settings,
                DesktopAppId::Terminal,
                DesktopAppId::Files,
                DesktopAppId::Monitor,
                DesktopAppId::SoftwareCenter,
                DesktopAppId::Media,
            ],
            count: 6,
        };
        assert_eq!(open_selection(&model, Some(DesktopAppId::Settings)), 1);
        assert_eq!(
            open_selection(&model, Some(DesktopAppId::SoftwareCenter)),
            5
        );
        assert_eq!(open_selection(&model, None), 0);
    }

    #[test]
    fn single_and_empty_models_commit_without_cycle() {
        let single = SwitcherModel {
            candidates: [DesktopAppId::Monitor; APP_COUNT],
            count: 1,
        };
        assert_eq!(single.target(7), Some(DesktopAppId::Monitor));
        assert_eq!(open_selection(&single, Some(DesktopAppId::Monitor)), 0);
        assert_eq!(SwitcherModel::EMPTY.target(0), None);
    }
}
