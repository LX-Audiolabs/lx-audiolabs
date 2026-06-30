use nice_plug::editor::Editor;
use nice_plug_iced::{NiceGuiContext, WindowState};
use std::sync::Arc;

/// Trait for editor apps that work with the LX Factory.
///
/// Implement this in your plugin's editor and call `create_lx_editor::<YourApp>(...)`.
///
/// Type Parameters:
/// - `Params`: Plugin-specific parameter struct (e.g., Arc<EquilibriumParams>)
/// - `State`: Shared runtime state (e.g., Arc<SharedState>)
pub trait LxEditorApp: Send + 'static {
    /// Associated message type (must impl Clone + Send + 'static).
    type Message: Clone + Send + 'static;

    /// Plugin parameters (must be Clone + Send + Sync for closure capture across threads).
    type Params: Clone + Send + Sync + 'static;

    /// Shared runtime state (must be Clone + Send + Sync for closure capture across threads).
    type State: Clone + Send + Sync + 'static;

    /// Boot the editor state with plugin-specific parameters.
    fn boot(
        nice_ctx: NiceGuiContext,
        params: Self::Params,
        state: Self::State,
    ) -> (Self, nice_plug_iced::iced::Task<Self::Message>)
    where
        Self: Sized;

    /// Update handler — called on every message.
    fn update(&mut self, message: Self::Message) -> nice_plug_iced::iced::Task<Self::Message>;

    /// View handler — render the UI.
    fn view(&self) -> nice_plug_iced::iced::Element<'_, Self::Message>;

    /// Create a Tick message for subscriptions.
    /// The factory calls this to drive GUI updates.
    fn tick() -> Self::Message;

    /// GUI tick/redraw interval in milliseconds. Default 30 ms (≈ 33 FPS) suits
    /// plugins with animated meters/spectra. Override (raise) for a near-static
    /// UI to save GPU — e.g. Aether sets 33 ms (≈ 30 FPS).
    const TICK_INTERVAL_MS: u64 = 30;
}

/// Create an editor using the LX factory pattern with dependency injection.
///
/// Params:
/// - `window_state`: Window handle from nice-plug
/// - `params`: Plugin-specific parameters to pass to boot()
/// - `state`: Shared runtime state to pass to boot()
/// - `always_redraw`: Pass true for animated UIs (FFT, meters, goniometer)
///
/// Returns None if editor creation fails.
pub fn create_lx_editor<T: LxEditorApp + Send + 'static>(
    window_state: Arc<WindowState>,
    params: T::Params,
    state: T::State,
    always_redraw: bool,
) -> Option<Box<dyn Editor>>
where
    T::Message: Send,
{
    let notifier = nice_plug_iced::iced::PollSubNotifier::default();
    let settings = nice_plug_iced::EditorSettings {
        ignore_non_modifier_keys: false,
        always_redraw,
    };

    nice_plug_iced::create_iced_editor(
        window_state,
        (),
        notifier,
        settings,
        move |editor_state, nice_ctx| {
            let params_clone = params.clone();
            let state_clone = state.clone();
            nice_plug_iced::application(
                editor_state,
                nice_ctx.clone(),
                move |_, ctx| T::boot(ctx, params_clone.clone(), state_clone.clone()),
                T::update,
                T::view,
            )
            .subscription(move |_state| {
                nice_plug_iced::iced::window::frames().map(|_| T::tick())
            })
            .run()
        },
    )
}
