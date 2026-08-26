use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    Discovered,
    Resolving,
    Ready,
    Updating,
    Degraded { reason: String },
    Removing,
}

impl LifecycleState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Removing)
    }

    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Ready | Self::Updating)
    }
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovered => f.write_str("discovered"),
            Self::Resolving => f.write_str("resolving"),
            Self::Ready => f.write_str("ready"),
            Self::Updating => f.write_str("updating"),
            Self::Degraded { .. } => f.write_str("degraded"),
            Self::Removing => f.write_str("removing"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transition {
    OwnerResolved,
    Resolved,
    ResolveFailed(String),
    Refresh,
    OwnerLost,
}

impl LifecycleState {
    pub fn apply(&self, transition: &Transition) -> Self {
        if matches!(self, Self::Removing) {
            return Self::Removing;
        }

        match transition {
            Transition::OwnerLost => Self::Removing,
            Transition::OwnerResolved => Self::Resolving,
            Transition::Resolved => Self::Ready,
            Transition::ResolveFailed(reason) => Self::Degraded {
                reason: reason.clone(),
            },
            Transition::Refresh => match self {
                Self::Discovered | Self::Resolving => Self::Resolving,
                Self::Degraded { reason } => Self::Degraded {
                    reason: reason.clone(),
                },
                Self::Ready | Self::Updating => Self::Updating,
                Self::Removing => Self::Removing,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_reaches_ready() {
        let state = LifecycleState::Discovered
            .apply(&Transition::OwnerResolved)
            .apply(&Transition::Resolved);
        assert_eq!(state, LifecycleState::Ready);
        assert!(state.is_resolved());
        assert!(state.is_visible());
    }

    #[test]
    fn refresh_of_a_ready_item_is_an_update() {
        let state = LifecycleState::Ready.apply(&Transition::Refresh);
        assert_eq!(state, LifecycleState::Updating);
        assert!(state.is_resolved());
    }

    #[test]
    fn failure_degrades_but_keeps_the_item_visible() {
        let state = LifecycleState::Resolving.apply(&Transition::ResolveFailed("timed out".into()));
        assert!(matches!(state, LifecycleState::Degraded { .. }));
        assert!(state.is_visible());
        assert!(!state.is_resolved());
    }

    #[test]
    fn removing_is_terminal() {
        let removed = LifecycleState::Ready.apply(&Transition::OwnerLost);
        assert_eq!(removed, LifecycleState::Removing);
        assert!(!removed.is_visible());

        for transition in [
            Transition::OwnerResolved,
            Transition::Resolved,
            Transition::Refresh,
            Transition::ResolveFailed("late".into()),
        ] {
            assert_eq!(removed.apply(&transition), LifecycleState::Removing);
        }
    }
}
