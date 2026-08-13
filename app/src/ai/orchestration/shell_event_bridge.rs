//! Orchestration shell event bridge — translates Warp's OSC 133 / DCS hook
//! events into the orchestration plane's `DcsHookEvent` variants.
//!
//! ## Event mapping
//!
//! | Warp Event                              | DcsHookEvent              |
//! |-----------------------------------------|---------------------------|
//! | AnsiHandlerEvent::Bootstrapped          | Bootstrapped              |
//! | AnsiHandlerEvent::Precmd                | Precmd (idle at prompt)   |
//! | AnsiHandlerEvent::Preexec               | PromptStarted (running)   |
//! | ModelEvent::AfterBlockCompleted         | CommandFinished { exit }  |
//!
//! ## Exit code propagation
//!
//! `AfterBlockCompletedEvent` now carries `exit_code: i32` (added in this
//! commit). The bridge reads it directly — no optimistic assumption.
//!
//! ## Architecture
//!
//! The bridge is a GPUI model that subscribes to `ModelEventDispatcher`.
//! It holds a `SessionDispatchMap` (session→dispatch_id) and an active
//! session tracker. On each event, it translates to `DcsHookEvent` and
//! calls `transition_worker` on the orchestration store.

use std::collections::HashMap;
use std::sync::Arc;


use ::ai::agent::orchestration::executor::DcsHookEvent;
use ::ai::agent::orchestration::OrchestrationStore;

use parking_lot::Mutex;
use crate::terminal::model_events::{AnsiHandlerEvent, ModelEvent, ModelEventDispatcher};
use crate::terminal::event::{AfterBlockCompletedEvent, BlockType, UserBlockCompleted};
use crate::terminal::model::session::SessionId;

/// Maps Warp session IDs to orchestration dispatch IDs.
/// Populated when a terminal pane is assigned to a worker dispatch.
///
/// Registered as a GPUI singleton; the inner state is an `Arc<Mutex<..>>`
/// so the `ShellEventBridge` can share it across model instances.
#[derive(Default, Clone)]
pub struct SessionDispatchMap {
    map: Arc<Mutex<HashMap<SessionId, String>>>,
}

impl SessionDispatchMap {
    pub fn register(&self, session_id: SessionId, dispatch_id: impl Into<String>) {
        self.map.lock().insert(session_id, dispatch_id.into());
    }

    pub fn unregister(&self, session_id: &SessionId) {
        self.map.lock().remove(session_id);
    }

    pub fn get(&self, session_id: &SessionId) -> Option<String> {
        self.map.lock().get(session_id).cloned()
    }

    /// Remove entries via predicate over (session_id, dispatch_id).
    pub fn retain<F: FnMut(SessionId, &str) -> bool>(&self, mut f: F) {
        self.map.lock().retain(|sid, did| f(*sid, did));
    }


    /// Get a cloned handle sharing the same inner map.
    pub fn handle_clone(&self) -> SessionDispatchMap {
        self.clone()
    }
}

impl warpui::Entity for SessionDispatchMap {
    type Event = ();
}

impl warpui::SingletonEntity for SessionDispatchMap {}

/// Shell event bridge — subscribes to ModelEventDispatcher and translates
/// shell events into worker state transitions.
///
/// Created once per terminal pane (or as a global singleton in future).
pub struct ShellEventBridge {
    dispatch_map: SessionDispatchMap,
    /// Track the currently active session for events that don't carry session_id.
    active_session_id: parking_lot::Mutex<Option<SessionId>>,
    /// The terminal view this bridge serves — injected after view creation.
    /// Enables automatic session-mailbox registration (Orca semantics: the
    /// terminal handle IS the mailbox for cross-harness direct sends).
    weak_view: parking_lot::Mutex<Option<warpui::WeakViewHandle<crate::terminal::view::TerminalView>>>,
}
impl ShellEventBridge {
    pub fn new(dispatch_map: SessionDispatchMap) -> Self {
        Self {
            dispatch_map,
            active_session_id: parking_lot::Mutex::new(None),
            weak_view: parking_lot::Mutex::new(None),
        }
    }

    /// Attach the terminal view (call after view creation).
    pub fn set_view(&self, view: warpui::WeakViewHandle<crate::terminal::view::TerminalView>) {
        log::debug!("orchestration: bridge set_view called");
        *self.weak_view.lock() = Some(view);
    }

    /// Mailbox handle for a session — the direct-send address of a terminal.
    /// Messages enqueued `to_handle = session_{sid}` are pointer-pushed into
    /// this terminal when its agent is idle (delivery.rs), and pulled via
    /// `check-messages session_{sid}`.
    pub fn session_mailbox_handle(session_id: SessionId) -> String {
        format!("session_{}", session_id.as_u64())
    }

    /// Idempotently register the session mailbox (ViewRegistry + push
    /// delivery) on first observation of the session. Runs on the GPUI
    /// main thread from the model-event subscription.
    pub fn ensure_session_mailbox(&self, session_id: SessionId, cx: &warpui::AppContext) {
        let Some(weak) = self.weak_view.lock().clone() else {
            return;
        };
        let Some(_view) = weak.upgrade(cx) else {
            return;
        };
        let mailbox = Self::session_mailbox_handle(session_id);
        use warpui::SingletonEntity as _;
        let already = crate::ai::orchestration::ViewRegistry::handle(cx)
            .read(cx, |registry, _| registry.get(&mailbox, cx).is_some());
        if !already {
            crate::ai::orchestration::ViewRegistry::handle(cx)
                .read(cx, |registry, _| registry.register(&mailbox, weak));
            ::ai::agent::orchestration::delivery::register_dispatch(&mailbox);
            log::info!("orchestration: session mailbox {mailbox} registered");
        }
    }

    /// Update the active session ID. Called when a Precmd event (which carries
    /// session_id) is observed.
    pub fn set_active_session(&self, session_id: SessionId) {
        *self.active_session_id.lock() = Some(session_id);
    }

    /// Resolve the dispatch_id for the active session.
    fn dispatch_for_active(&self) -> Option<String> {
        let guard = self.active_session_id.lock();
        guard.as_ref().and_then(|sid| self.dispatch_map.get(sid))
    }

    /// Process a `ModelEvent` and return the corresponding `(dispatch_id, DcsHookEvent)`
    /// if one was generated. Returns `None` for irrelevant events or when no
    /// dispatch is registered for the active session.
    pub fn translate_event(&self, event: &ModelEvent) -> Option<(String, DcsHookEvent)> {
        match event {
            // Bootstrapped carries session_id — direct lookup.
            ModelEvent::Handler(AnsiHandlerEvent::Bootstrapped { session_id, .. }) => {
                let dispatch_id = self.dispatch_map.get(session_id)?;
                Some((dispatch_id, DcsHookEvent::Bootstrapped { shell_path: None }))
            }

            // Precmd carries session_id via the Handler event, but AnsiHandlerEvent::Precmd
            // itself doesn't include it. We track it via set_active_session() called
            // externally (or via ModelEventDispatcher subscription).
            ModelEvent::Handler(AnsiHandlerEvent::Precmd) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((dispatch_id, DcsHookEvent::Precmd))
            }

            // Preexec = command started.
            ModelEvent::Handler(AnsiHandlerEvent::Preexec) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((dispatch_id, DcsHookEvent::PromptStarted))
            }

            // AfterBlockCompleted carries the real exit_code.
            ModelEvent::AfterBlockCompleted(ev) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((
                    dispatch_id,
                    DcsHookEvent::CommandFinished {
                        exit_code: ev.exit_code,
                    },
                ))
            }

            _ => None,
        }
    }
}

/// GPUI Entity implementation — ShellEventBridge is a passive observer
/// model. It emits no events of its own.
impl warpui::Entity for ShellEventBridge {
    type Event = ();
}

/// Subscribe the bridge to a `ModelEventDispatcher`. On each terminal model
/// event, translate it to a `DcsHookEvent` and apply the worker state
/// transition via the orchestration store.
pub fn subscribe_bridge(
    bridge: &warpui::ModelHandle<ShellEventBridge>,
    model_events: &warpui::ModelHandle<ModelEventDispatcher>,
    ctx: &mut warpui::AppContext,
) {
    bridge.update(ctx, |_, ctx| {
        ctx.subscribe_to_model(model_events, |bridge, event, ctx| {
            // Track active session from events that carry session_id, and
            // idempotently register the session mailbox for direct sends.
            if let ModelEvent::Handler(AnsiHandlerEvent::Bootstrapped { session_id, .. }) = event {
                bridge.set_active_session(*session_id);
                bridge.ensure_session_mailbox(*session_id, ctx);
            }

            // PTY exit → retire the session mailbox (Orca
            // retirePendingMessageDeliveryForPty: clear flight + watermark
            // so a same-id reborn PTY never receives a stale Enter).
            if let ModelEvent::ExitShell { session_id } = event {
                let mailbox = ShellEventBridge::session_mailbox_handle(*session_id);
                ::ai::agent::orchestration::delivery::unregister_dispatch(&mailbox);
                crate::ai::orchestration::session_activity::remove(*session_id);
                use warpui::SingletonEntity as _;
                crate::ai::orchestration::ViewRegistry::handle(ctx)
                    .read(ctx, |registry, _| registry.unregister(&mailbox));
                log::info!("orchestration: session mailbox {mailbox} retired (shell exit)");
            }

            // Publish shell lifecycle timestamps to the session-activity
            // registry — the idle probe's source for since_last_precmd_ms
            // and output silence (idle_detector multi-signal path).
            if let Some(sid) = *bridge.active_session_id.lock() {
                match event {
                    ModelEvent::Handler(AnsiHandlerEvent::Precmd) => {
                        crate::ai::orchestration::session_activity::record_precmd(sid);
                    }
                    ModelEvent::Handler(AnsiHandlerEvent::Preexec)
                    | ModelEvent::AfterBlockCompleted(_) => {
                        crate::ai::orchestration::session_activity::record_event(sid);
                    }
                    _ => {}
                }
            }


            // ── Block-driven settlement (direction 2) ──────────────
            // If the completed block's command matches the dispatch's
            // start_options.command, settle immediately — independent of
            // preamble detection.
            if let ModelEvent::AfterBlockCompleted(AfterBlockCompletedEvent {
                exit_code,
                block_type: BlockType::User(UserBlockCompleted { command, .. }),
                ..
            }) = event
            {
                if let Some(dispatch_id) = bridge.dispatch_for_active() {
                    let store = ::ai::agent::orchestration::connection::store();
                    let settled = crate::ai::orchestration::block_settle::try_settle_from_block(
                        &dispatch_id,
                        command,
                        *exit_code,
                        store,
                    );
                    if settled {
                        log::info!(
                            "orchestration: block_settle settled dispatch {} (exit={exit_code}, cmd={command:?})",
                            dispatch_id
                        );
                    }
                }
            }

            // Translate and apply.
            if let Some((dispatch_id, dcs_event)) = bridge.translate_event(event) {
                let target = dcs_event.target_state();
                if let Some(next_state) = target {
                    let store = ::ai::agent::orchestration::connection::store();
                    if let Err(e) = store.transition_worker(&dispatch_id, next_state) {
                        log::warn!(
                            "orchestration: worker transition failed for {}: {e}",
                            dispatch_id
                        );
                    }
                }
            }
            // No need to notify — the bridge is a passive observer.
            let _ = ctx;
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_dispatch_map_register_and_get() {
        let map = SessionDispatchMap::default();
        let sid = SessionId::from(42);
        map.register(sid, "dispatch_123");
        assert_eq!(map.get(&sid), Some("dispatch_123".to_string()));
        map.unregister(&sid);
        assert_eq!(map.get(&sid), None);
    }
}
