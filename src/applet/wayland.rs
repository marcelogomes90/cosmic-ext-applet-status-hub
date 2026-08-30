use std::collections::HashMap;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cosmic::cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1;
use cosmic::cctk::cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1;
use cosmic::cctk::sctk;
use cosmic::cctk::sctk::activation::{
    ActivationHandler, ActivationState, RequestData, RequestDataExt,
};
use cosmic::cctk::sctk::reexports::calloop;
use cosmic::cctk::sctk::reexports::calloop_wayland_source::WaylandSource;
use cosmic::cctk::sctk::registry::{ProvidesRegistryState, RegistryState};
use cosmic::cctk::sctk::seat::{SeatHandler, SeatState};
use cosmic::cctk::toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState};
use cosmic::cctk::toplevel_management::{ToplevelManagerHandler, ToplevelManagerState};
use cosmic::cctk::wayland_client::globals::registry_queue_init;
use cosmic::cctk::wayland_client::protocol::wl_seat::WlSeat;
use cosmic::cctk::wayland_client::protocol::wl_surface::WlSurface;
use cosmic::cctk::wayland_client::{Connection, QueueHandle, WEnum};
use cosmic::cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use cosmic::iced::Subscription;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::StreamExt;

use crate::applet::identity;

pub const SETTLE: Duration = Duration::from_millis(400);

const SOCKET: &str = "X_PRIVILEGED_WAYLAND_SOCKET";

#[derive(Clone, Debug)]
pub struct TokenRequest {
    pub app_id: String,
    pub exec: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raise {
    Unfocused,
    Minimized,
    Changed,
}

#[derive(Clone, Debug)]
pub struct ActivateRequest {
    pub hints: Vec<String>,
    pub raise: Raise,
}

#[derive(Clone, Debug)]
pub enum WaylandRequest {
    Token(TokenRequest),
    Activate(ActivateRequest),
}

#[derive(Clone, Debug)]
pub enum WaylandUpdate {
    Init(calloop::channel::Sender<WaylandRequest>),
    Finished,
    ActivationToken { token: Option<String>, exec: String },
}

pub fn subscription() -> Subscription<WaylandUpdate> {
    Subscription::run_with("cosmic-status-hub-wayland", |_| updates())
}

enum State {
    Ready,
    Running(UnboundedReceiver<WaylandUpdate>),
    Finished,
}

fn updates() -> impl futures::Stream<Item = WaylandUpdate> + Send + 'static {
    futures::stream::unfold(State::Ready, |state| async move {
        match state {
            State::Ready => {
                let (requests_tx, requests_rx) = calloop::channel::channel();
                let (updates_tx, updates_rx) = unbounded();
                std::thread::spawn(move || run(&updates_tx, requests_rx));
                Some((WaylandUpdate::Init(requests_tx), State::Running(updates_rx)))
            }
            State::Running(mut updates) => match updates.next().await {
                Some(update) => Some((update, State::Running(updates))),
                None => Some((WaylandUpdate::Finished, State::Finished)),
            },
            State::Finished => {
                futures::future::pending::<()>().await;
                None
            }
        }
    })
}

struct ExecRequestData {
    data: RequestData,
    exec: String,
}

impl RequestDataExt for ExecRequestData {
    fn app_id(&self) -> Option<&str> {
        self.data.app_id()
    }

    fn seat_and_serial(&self) -> Option<(&WlSeat, u32)> {
        self.data.seat_and_serial()
    }

    fn surface(&self) -> Option<&WlSurface> {
        self.data.surface()
    }
}

struct Pending {
    id: u64,
    hints: Vec<String>,
    raise: Raise,
    before: HashMap<String, bool>,
}

struct AppData {
    exit: bool,
    updates: UnboundedSender<WaylandUpdate>,
    queue_handle: QueueHandle<Self>,
    loop_handle: calloop::LoopHandle<'static, Self>,
    registry_state: RegistryState,
    seat_state: SeatState,
    activation_state: Option<ActivationState>,
    toplevel_info: Option<ToplevelInfoState>,
    toplevel_manager: Option<ToplevelManagerState>,
    pending: Vec<Pending>,
    next_pending: u64,
}

impl AppData {
    fn on_request(&mut self, event: calloop::channel::Event<WaylandRequest>) {
        match event {
            calloop::channel::Event::Msg(WaylandRequest::Token(request)) => {
                self.request_token(request);
            }
            calloop::channel::Event::Msg(WaylandRequest::Activate(request)) => {
                self.schedule(request);
            }
            calloop::channel::Event::Closed => self.exit = true,
        }
    }

    fn request_token(&mut self, request: TokenRequest) {
        let Some(activation) = self.activation_state.as_ref() else {
            let _ = self.updates.unbounded_send(WaylandUpdate::ActivationToken {
                token: None,
                exec: request.exec,
            });
            return;
        };

        activation.request_token_with_data(
            &self.queue_handle,
            ExecRequestData {
                data: RequestData {
                    app_id: Some(request.app_id),
                    seat_and_serial: self.seat_state.seats().next().map(|seat| (seat, 0)),
                    surface: None,
                },
                exec: request.exec,
            },
        );
    }

    fn schedule(&mut self, request: ActivateRequest) {
        if self.toplevel_info.is_none() || self.toplevel_manager.is_none() {
            return;
        }

        let before = self
            .matching(&request.hints)
            .into_iter()
            .map(|info| (info.identifier.clone(), minimized(info)))
            .collect();

        self.next_pending = self.next_pending.wrapping_add(1);
        let id = self.next_pending;
        self.pending.push(Pending {
            id,
            hints: request.hints,
            raise: request.raise,
            before,
        });

        let timer = calloop::timer::Timer::from_duration(SETTLE);
        if self
            .loop_handle
            .insert_source(timer, move |_, (), state| {
                state.settle(id);
                calloop::timer::TimeoutAction::Drop
            })
            .is_err()
        {
            self.pending.retain(|pending| pending.id != id);
        }
    }

    fn settle(&mut self, id: u64) {
        let Some(index) = self.pending.iter().position(|pending| pending.id == id) else {
            return;
        };
        let pending = self.pending.remove(index);

        let Some((identifier, handle)) = self
            .pick(&pending)
            .and_then(|info| Some((info.identifier.clone(), info.cosmic_toplevel.clone()?)))
        else {
            return;
        };
        let Some(seat) = self.seat_state.seats().next() else {
            return;
        };
        let Some(manager) = self.toplevel_manager.as_ref() else {
            return;
        };

        tracing::info!(window = %identifier, "raising the window the tray item stands for");
        manager.manager.activate(&handle, &seat);
    }

    fn pick(&self, pending: &Pending) -> Option<&ToplevelInfo> {
        let matches = self.matching(&pending.hints);
        let candidates: Vec<Candidate<'_>> = matches
            .iter()
            .map(|info| Candidate {
                identifier: info.identifier.as_str(),
                minimized: minimized(info),
                activated: activated(info),
            })
            .collect();

        choose(&pending.before, &candidates, pending.raise).map(|index| matches[index])
    }

    fn matching(&self, hints: &[String]) -> Vec<&ToplevelInfo> {
        if hints.is_empty() {
            return Vec::new();
        }
        let Some(state) = self.toplevel_info.as_ref() else {
            return Vec::new();
        };

        let mut ranked: Vec<(u32, &ToplevelInfo)> = state
            .toplevels()
            .filter(|info| info.cosmic_toplevel.is_some())
            .filter_map(|info| {
                let score = identity::score(hints, &info.app_id, &info.title);
                (score >= identity::MATCH_THRESHOLD).then_some((score, info))
            })
            .collect();
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        ranked.into_iter().map(|(_, info)| info).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Candidate<'a> {
    identifier: &'a str,
    minimized: bool,
    activated: bool,
}

fn choose(
    before: &HashMap<String, bool>,
    candidates: &[Candidate<'_>],
    raise: Raise,
) -> Option<usize> {
    if let Some(appeared) = candidates
        .iter()
        .position(|candidate| !before.contains_key(candidate.identifier))
    {
        return Some(appeared);
    }

    if let Some(restored) = candidates.iter().position(|candidate| {
        before.get(candidate.identifier) == Some(&true) && !candidate.minimized
    }) {
        return Some(restored);
    }

    let hidden = candidates
        .iter()
        .any(|candidate| before.get(candidate.identifier) == Some(&false) && candidate.minimized);
    let closed = before.keys().any(|identifier| {
        !candidates
            .iter()
            .any(|candidate| candidate.identifier == identifier)
    });

    if hidden || closed {
        return None;
    }

    match raise {
        Raise::Unfocused => candidates.iter().position(|candidate| !candidate.activated),
        Raise::Minimized => candidates.iter().position(|candidate| candidate.minimized),
        Raise::Changed => None,
    }
}

fn minimized(info: &ToplevelInfo) -> bool {
    info.state
        .contains(&zcosmic_toplevel_handle_v1::State::Minimized)
}

fn activated(info: &ToplevelInfo) -> bool {
    info.state
        .contains(&zcosmic_toplevel_handle_v1::State::Activated)
}

fn connect() -> Option<Connection> {
    let socket = std::env::var(SOCKET).ok().and_then(|fd| {
        fd.parse::<RawFd>()
            .ok()
            .map(|fd| unsafe { UnixStream::from_raw_fd(fd) })
    });

    let connection = match socket {
        Some(socket) => Connection::from_socket(socket),
        None => Connection::connect_to_env(),
    };

    match connection {
        Ok(connection) => Some(connection),
        Err(why) => {
            tracing::warn!(?why, "no Wayland connection for window activation");
            None
        }
    }
}

fn run(
    updates: &UnboundedSender<WaylandUpdate>,
    requests: calloop::channel::Channel<WaylandRequest>,
) {
    let Some(connection) = connect() else {
        return;
    };
    let Ok((globals, event_queue)) = registry_queue_init::<AppData>(&connection) else {
        tracing::warn!("the Wayland registry did not answer");
        return;
    };
    let Ok(mut event_loop) = calloop::EventLoop::<AppData>::try_new() else {
        return;
    };

    let queue_handle = event_queue.handle();
    let loop_handle = event_loop.handle();

    if WaylandSource::new(connection, event_queue)
        .insert(loop_handle.clone())
        .is_err()
    {
        return;
    }
    if loop_handle
        .insert_source(requests, |event, (), state| state.on_request(event))
        .is_err()
    {
        return;
    }

    let registry_state = RegistryState::new(&globals);
    let mut app_data = AppData {
        exit: false,
        updates: updates.clone(),
        seat_state: SeatState::new(&globals, &queue_handle),
        activation_state: ActivationState::bind::<AppData>(&globals, &queue_handle).ok(),
        toplevel_info: ToplevelInfoState::try_new(&registry_state, &queue_handle),
        toplevel_manager: ToplevelManagerState::try_new(&registry_state, &queue_handle),
        registry_state,
        loop_handle,
        queue_handle,
        pending: Vec::new(),
        next_pending: 0,
    };

    tracing::info!(
        activation = app_data.activation_state.is_some(),
        toplevels = app_data.toplevel_info.is_some(),
        manager = app_data.toplevel_manager.is_some(),
        "privileged Wayland connection ready"
    );

    loop {
        if app_data.exit {
            break;
        }
        if let Err(why) = event_loop.dispatch(None, &mut app_data) {
            if let calloop::Error::IoError(ref io) = why
                && io.kind() == std::io::ErrorKind::BrokenPipe
            {
                tracing::info!("the connection to the panel ended");
            } else {
                tracing::error!(?why, "the privileged Wayland connection failed");
            }
            break;
        }
    }
}

impl ProvidesRegistryState for AppData {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    sctk::registry_handlers!();
}

impl ActivationHandler for AppData {
    type RequestData = ExecRequestData;

    fn new_token(&mut self, token: String, data: &ExecRequestData) {
        let _ = self.updates.unbounded_send(WaylandUpdate::ActivationToken {
            token: Some(token),
            exec: data.exec.clone(),
        });
    }
}

impl SeatHandler for AppData {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        _: sctk::seat::Capability,
    ) {
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        _: sctk::seat::Capability,
    ) {
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl ToplevelInfoHandler for AppData {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        self.toplevel_info
            .as_mut()
            .expect("toplevel events only arrive once the global is bound")
    }

    fn new_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
    }

    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
    }

    fn toplevel_closed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ExtForeignToplevelHandleV1,
    ) {
    }
}

impl ToplevelManagerHandler for AppData {
    fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
        self.toplevel_manager
            .as_mut()
            .expect("manager events only arrive once the global is bound")
    }

    fn capabilities(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: Vec<WEnum<zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>>,
    ) {
    }
}

sctk::delegate_activation!(AppData, ExecRequestData);
sctk::delegate_seat!(AppData);
sctk::delegate_registry!(AppData);
cosmic::cctk::delegate_toplevel_info!(AppData);
cosmic::cctk::delegate_toplevel_manager!(AppData);

#[cfg(test)]
mod tests {
    use super::*;

    fn before<const N: usize>(entries: [(&str, bool); N]) -> HashMap<String, bool> {
        entries
            .into_iter()
            .map(|(identifier, minimized)| (identifier.to_owned(), minimized))
            .collect()
    }

    fn window(identifier: &str, minimized: bool, activated: bool) -> Candidate<'_> {
        Candidate {
            identifier,
            minimized,
            activated,
        }
    }

    #[test]
    fn a_window_that_only_the_compositor_can_restore_is_raised_on_a_left_click() {
        let now = [window("slack", true, false)];

        assert_eq!(
            choose(&before([("slack", true)]), &now, Raise::Unfocused),
            Some(0)
        );
    }

    #[test]
    fn an_application_that_hid_itself_is_left_alone() {
        let now = [window("slack", true, false)];

        for raise in [Raise::Unfocused, Raise::Minimized, Raise::Changed] {
            assert_eq!(choose(&before([("slack", false)]), &now, raise), None);
        }
    }

    #[test]
    fn a_window_that_closed_is_not_replaced_by_another_one() {
        let now = [window("second", false, false)];

        assert_eq!(
            choose(
                &before([("first", false), ("second", false)]),
                &now,
                Raise::Unfocused
            ),
            None
        );
    }

    #[test]
    fn a_window_the_application_just_opened_wins_over_the_older_ones() {
        let now = [window("old", true, false), window("new", false, false)];

        for raise in [Raise::Unfocused, Raise::Minimized, Raise::Changed] {
            assert_eq!(choose(&before([("old", true)]), &now, raise), Some(1));
        }
    }

    #[test]
    fn a_window_already_in_front_is_left_where_it_is() {
        let now = [window("slack", false, true)];

        for raise in [Raise::Unfocused, Raise::Minimized, Raise::Changed] {
            assert_eq!(choose(&before([("slack", false)]), &now, raise), None);
        }
    }

    #[test]
    fn a_toggle_or_submenu_never_raises_a_settled_window() {
        let visible = [window("obs", false, false)];
        let hidden = [window("obs", true, false)];

        assert_eq!(
            choose(&before([("obs", false)]), &visible, Raise::Changed),
            None
        );
        assert_eq!(
            choose(&before([("obs", true)]), &hidden, Raise::Changed),
            None
        );
    }

    #[test]
    fn a_menu_entry_reaches_a_minimized_window_only() {
        let visible = [window("heroic", false, false)];
        let hidden = [window("heroic", true, false)];

        assert_eq!(
            choose(&before([("heroic", false)]), &visible, Raise::Minimized),
            None
        );
        assert_eq!(
            choose(&before([("heroic", true)]), &hidden, Raise::Minimized),
            Some(0)
        );
    }

    #[test]
    fn a_left_click_still_reaches_a_window_that_is_merely_out_of_focus() {
        let now = [window("slack", false, false)];

        assert_eq!(
            choose(&before([("slack", false)]), &now, Raise::Unfocused),
            Some(0)
        );
    }

    #[test]
    fn an_application_that_restored_its_own_window_is_still_brought_forward() {
        let now = [window("slack", false, false)];

        for raise in [Raise::Unfocused, Raise::Minimized, Raise::Changed] {
            assert_eq!(choose(&before([("slack", true)]), &now, raise), Some(0));
        }
    }

    #[test]
    fn nothing_matching_means_nothing_to_raise() {
        for raise in [Raise::Unfocused, Raise::Minimized, Raise::Changed] {
            assert_eq!(choose(&before([]), &[], raise), None);
        }
    }
}
