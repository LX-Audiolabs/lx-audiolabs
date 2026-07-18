use std::cell::RefCell;

use crate::application::ApplicationRunner;
use baseview::{
    dpi::LogicalSize, Event, EventStatus, Window, WindowContext, WindowHandler, WindowHandle,
    WindowOpenOptions, WindowScalePolicy,
};
use baseview::gl::GlConfig;
use gl::types::GLint;
use gl_rs as gl;
use skia_safe::gpu::gl::FramebufferInfo;
use skia_safe::gpu::{
    self, backend_render_targets, ganesh::context_options, ContextOptions, SurfaceOrigin,
};
use skia_safe::{ColorSpace, ColorType, PixelGeometry, Surface, SurfaceProps, SurfacePropsFlags};

use crate::proxy::BaseviewProxy;
use vizia_core::backend::*;
use vizia_core::prelude::*;

/// Handles a vizia_baseview application
pub(crate) struct ViziaWindow {
    application: RefCell<ApplicationRunner>,
    window_context: WindowContext,
    #[allow(clippy::type_complexity)]
    on_idle: Option<Box<dyn Fn(&mut Context) + Send>>,
}

impl ViziaWindow {
    fn new(
        mut cx: BackendContext,
        win_desc: WindowDescription,
        window_scale_policy: WindowScalePolicy,
        window_context: &WindowContext,
        builder: Option<Box<dyn FnOnce(&mut Context) + Send>>,
        on_idle: Option<Box<dyn Fn(&mut Context) + Send>>,
        is_parented: bool,
    ) -> ViziaWindow {
        Runtime::init_on_ui_thread();
        Runtime::set_sync_effect_waker(|| {});

        let context = window_context
            .gl_context()
            .expect("Window was created without OpenGL support");

        unsafe { context.make_current() };

        gl::load_with(|s| context.get_proc_address(s));
        let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
            if name == "eglGetCurrentDisplay" {
                return std::ptr::null();
            }
            context.get_proc_address(name)
        })
        .expect("Could not create interface");

        let mut context_options = ContextOptions::new();
        context_options.skip_gl_error_checks = context_options::Enable::Yes;

        let mut gr_context = skia_safe::gpu::direct_contexts::make_gl(interface, &context_options)
            .expect("Could not create direct context");

        let fb_info = {
            let mut fboid: GLint = 0;
            unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fboid) };

            FramebufferInfo {
                fboid: fboid.try_into().unwrap(),
                format: skia_safe::gpu::gl::Format::RGBA8.into(),
                ..Default::default()
            }
        };

        let mut surface = create_surface(
            (win_desc.inner_size.width as i32, win_desc.inner_size.height as i32),
            fb_info,
            &mut gr_context,
        );

        let dirty_surface = surface
            .new_surface_with_dimensions((
                win_desc.inner_size.width as i32,
                win_desc.inner_size.height as i32,
            ))
            .unwrap();

        let (use_system_scaling, window_scale_factor) = match window_scale_policy {
            WindowScalePolicy::ScaleFactor(scale) => (false, scale),
            // 1.0 until the first DPI/resize event — a hard-coded 1.25 mis-scaled
            // layout and (with older mouse code) hit tests on 100% displays.
            WindowScalePolicy::SystemScaleFactor => (true, 1.0),
        };
        let dpi_factor = window_scale_factor * win_desc.user_scale_factor;

        cx.add_main_window(Entity::root(), &win_desc, dpi_factor as f32);
        cx.add_window(WindowView {});

        cx.0.windows.insert(Entity::root(), {
            let mut state = WindowState::default();
            state.window_description = win_desc.clone();
            state
        });

        cx.context().add_built_in_styles();
        if let Some(builder) = builder {
            (builder)(cx.context());
        }

        let application = ApplicationRunner::new(
            cx,
            gr_context,
            use_system_scaling,
            window_scale_factor,
            surface,
            dirty_surface,
            win_desc,
            is_parented,
            window_context.clone(),
        );
        unsafe { context.make_not_current() };

        ViziaWindow { application: RefCell::new(application), window_context: window_context.clone(), on_idle }
    }

    /// Open a new child window.
    pub fn open_parented<P, F>(
        parent: &P,
        win_desc: WindowDescription,
        scale_policy: WindowScalePolicy,
        app: F,
        on_idle: Option<Box<dyn Fn(&mut Context) + Send>>,
        ignore_default_theme: bool,
    ) -> WindowHandle
    where
        P: raw_window_handle::HasWindowHandle,
        F: Fn(&mut Context),
        F: 'static + Send,
    {
        // Parent child windows must not wait on swap-interval: DAW hosts
        // drive the UI thread; vsync can block forever with no visible UI.
        let window_settings = WindowOpenOptions::new()
            .with_title(win_desc.title.clone())
            .with_size(LogicalSize {
                width: win_desc.inner_size.width as f64 * win_desc.user_scale_factor,
                height: win_desc.inner_size.height as f64 * win_desc.user_scale_factor,
            })
            .with_scale_policy(scale_policy)
            .with_gl_config(GlConfig { vsync: false, ..GlConfig::default() });

        Window::open_parented(
            parent,
            window_settings,
            move |window_context: WindowContext| -> ViziaWindow {
                Runtime::init_on_ui_thread();
                let mut cx = Context::new();

                cx.ignore_default_theme = ignore_default_theme;
                cx.add_built_in_styles();

                let mut cx = BackendContext::new(cx);

                cx.set_event_proxy(Box::new(BaseviewProxy));
                ViziaWindow::new(
                    cx,
                    win_desc,
                    scale_policy,
                    &window_context,
                    Some(Box::new(app)),
                    on_idle,
                    true,
                )
            },
        )
    }

    /// Open a new window that blocks the current thread until the window is destroyed.
    pub fn open_blocking<F>(
        win_desc: WindowDescription,
        scale_policy: WindowScalePolicy,
        app: F,
        on_idle: Option<Box<dyn Fn(&mut Context) + Send>>,
        ignore_default_theme: bool,
    ) where
        F: Fn(&mut Context),
        F: 'static + Send,
    {
        let window_settings = WindowOpenOptions::new()
            .with_title(win_desc.title.clone())
            .with_size(LogicalSize {
                width: win_desc.inner_size.width as f64 * win_desc.user_scale_factor,
                height: win_desc.inner_size.height as f64 * win_desc.user_scale_factor,
            })
            .with_scale_policy(scale_policy)
            .with_gl_config(GlConfig { vsync: true, ..GlConfig::default() });

        Window::open_blocking(
            window_settings,
            move |window_context: WindowContext| -> ViziaWindow {
                Runtime::init_on_ui_thread();
                let mut cx = Context::new();

                cx.ignore_default_theme = ignore_default_theme;
                cx.add_built_in_styles();

                let mut cx = BackendContext::new(cx);

                cx.set_event_proxy(Box::new(BaseviewProxy));
                ViziaWindow::new(
                    cx,
                    win_desc,
                    scale_policy,
                    &window_context,
                    Some(Box::new(app)),
                    on_idle,
                    false,
                )
            },
        );
    }
}

impl WindowHandler for ViziaWindow {
    fn on_frame(&self) {
        Runtime::init_on_ui_thread();
        // WM_TIMER / Present / SetWindowPos can re-enter this wndproc while we
        // already hold the RefCell (e.g. init resize → WM_SIZE → resized()).
        // borrow_mut would panic across FFI and kill the DAW — skip re-entry.
        let Ok(mut application) = self.application.try_borrow_mut() else {
            return;
        };
        application.on_frame_update();
        application.handle_idle(&self.on_idle);
        application.render();
    }

    fn resized(&self, new_size: baseview::WindowSize) {
        Runtime::init_on_ui_thread();
        let Ok(mut application) = self.application.try_borrow_mut() else {
            return;
        };
        application.handle_resized(new_size);
    }

    fn on_event(&self, event: Event) -> EventStatus {
        Runtime::init_on_ui_thread();
        let mut should_quit = false;

        let captured = matches!(event, Event::Keyboard(_))
            && self
                .application
                .try_borrow()
                .map(|app| app.focused_element() == Some("textbox"))
                .unwrap_or(false);

        let Ok(mut application) = self.application.try_borrow_mut() else {
            return EventStatus::Ignored;
        };
        application.handle_event(event, &mut should_quit);
        application.handle_idle(&self.on_idle);

        if should_quit {
            self.window_context.request_close();
        }

        if captured { EventStatus::Captured } else { EventStatus::Ignored }
    }
}

impl Drop for ViziaWindow {
    fn drop(&mut self) {
        Runtime::deinit_on_ui_thread();
    }
}

pub struct WindowView {}

impl View for WindowView {}

pub fn create_surface(
    size: (i32, i32),
    fb_info: FramebufferInfo,
    gr_context: &mut skia_safe::gpu::DirectContext,
) -> Surface {
    let backend_render_target = backend_render_targets::make_gl(size, None, 8, fb_info);

    let surface_props = SurfaceProps::new_with_text_properties(
        SurfacePropsFlags::default(),
        PixelGeometry::default(),
        0.5,
        0.0,
    );

    gpu::surfaces::wrap_backend_render_target(
        gr_context,
        &backend_render_target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        ColorSpace::new_srgb(),
        Some(surface_props).as_ref(),
    )
    .expect("Could not create skia surface")
}
