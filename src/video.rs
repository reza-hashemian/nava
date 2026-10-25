//! Video: mpv draws each frame straight into an OpenGL texture on the GPU,
//! and Slint shows that texture like any other image. Nothing is copied
//! through the CPU, so the glass controls can float over the picture for free.
//!
//! Everything here runs on the UI thread inside Slint's rendering notifier,
//! where Slint's OpenGL context is current.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::num::NonZeroU32;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use glow::HasContext;

use crate::mpv::MpvHandle;

#[repr(C)]
struct RenderContext {
    _private: [u8; 0],
}

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

type GetProc = unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
struct GlInitParams {
    get_proc_address: Option<GetProc>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct GlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

const PARAM_END: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const PARAM_OPENGL_FBO: c_int = 3;
const PARAM_FLIP_Y: c_int = 4;
const PARAM_X11_DISPLAY: c_int = 8;
const UPDATE_FRAME: u64 = 1;

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_render_context_create(res: *mut *mut RenderContext, mpv: *mut MpvHandle, params: *mut RenderParam) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut RenderContext,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
        data: *mut c_void,
    );
    fn mpv_render_context_update(ctx: *mut RenderContext) -> u64;
    fn mpv_render_context_render(ctx: *mut RenderContext, params: *mut RenderParam) -> c_int;
    fn mpv_render_context_report_swap(ctx: *mut RenderContext);
    fn mpv_render_context_free(ctx: *mut RenderContext);
}

/// Set when mpv has a frame for us and a redraw is already on its way.
static FRAME_WAITING: AtomicBool = AtomicBool::new(false);

/// mpv calls this from its own thread when a new frame is ready.
unsafe extern "C" fn on_update(_: *mut c_void) {
    if !FRAME_WAITING.swap(true, Ordering::AcqRel) {
        let _ = slint::invoke_from_event_loop(crate::request_redraw);
    }
}

type Loader<'a> = &'a dyn Fn(&CStr) -> *const c_void;

unsafe extern "C" fn get_proc(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let loader = unsafe { &*(ctx as *const Loader) };
    loader(unsafe { CStr::from_ptr(name) }) as *mut c_void
}

pub struct VideoOut {
    ctx: *mut RenderContext,
    gl: glow::Context,
    fbo: glow::Framebuffer,
    /// Two textures, drawn into in turn. A different texture each frame tells
    /// Slint the picture changed; with one texture it keeps a cached frame.
    textures: [Option<glow::Texture>; 2],
    current: usize,
    /// Old textures after a resize, deleted a frame later because Slint may still be drawing them.
    retired: Vec<glow::Texture>,
    size: (u32, u32),
}

impl VideoOut {
    /// Must be called while Slint's OpenGL context is current.
    /// `x11_display` (an Xlib `Display*`) lets mpv hand decoded frames from the
    /// GPU decoder straight to OpenGL instead of copying them through memory.
    pub fn new(mpv: *mut MpvHandle, loader: &dyn Fn(&CStr) -> *const c_void, x11_display: *mut c_void) -> Option<VideoOut> {
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name| loader(name)) };
        let mut l: Loader = loader;
        let mut init = GlInitParams { get_proc_address: Some(get_proc), get_proc_address_ctx: &mut l as *mut Loader as *mut c_void };
        let mut params = [
            RenderParam { kind: PARAM_API_TYPE, data: c"opengl".as_ptr() as *mut c_void },
            RenderParam { kind: PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut GlInitParams as *mut c_void },
            RenderParam {
                kind: if x11_display.is_null() { PARAM_END } else { PARAM_X11_DISPLAY },
                data: x11_display,
            },
            RenderParam { kind: PARAM_END, data: ptr::null_mut() },
        ];
        let mut ctx = ptr::null_mut();
        if unsafe { mpv_render_context_create(&mut ctx, mpv, params.as_mut_ptr()) } < 0 {
            eprintln!("nava: video output unavailable");
            return None;
        }
        unsafe { mpv_render_context_set_update_callback(ctx, Some(on_update), ptr::null_mut()) };
        let fbo = unsafe { gl.create_framebuffer() }.ok()?;
        Some(VideoOut { ctx, gl, fbo, textures: [None, None], current: 0, retired: Vec::new(), size: (0, 0) })
    }

    /// Draw the newest frame at `w`×`h` pixels. Returns the image to show, or
    /// `None` when there was no new frame.
    pub fn render(&mut self, w: u32, h: u32) -> Option<slint::Image> {
        FRAME_WAITING.store(false, Ordering::Release);
        let flags = unsafe { mpv_render_context_update(self.ctx) };
        let resized = (w, h) != self.size;
        if w == 0 || h == 0 || (flags & UPDATE_FRAME == 0 && !resized) {
            return None;
        }
        let gl = &self.gl;
        unsafe {
            for t in self.retired.drain(..) {
                gl.delete_texture(t);
            }
            let prev_fbo = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let prev_tex = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
            let mut viewport = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);

            if resized || self.textures[0].is_none() {
                for slot in &mut self.textures {
                    if let Some(old) = slot.take() {
                        self.retired.push(old);
                    }
                    let t = gl.create_texture().ok()?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(t));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        w as i32,
                        h as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                    *slot = Some(t);
                }
                self.size = (w, h);
            }
            self.current ^= 1;
            let target = self.textures[self.current]?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(target), 0);

            let mut fbo = GlFbo { fbo: self.fbo.0.get() as c_int, w: w as c_int, h: h as c_int, internal_format: 0 };
            let mut flip: c_int = 0;
            let mut params = [
                RenderParam { kind: PARAM_OPENGL_FBO, data: &mut fbo as *mut GlFbo as *mut c_void },
                RenderParam { kind: PARAM_FLIP_Y, data: &mut flip as *mut c_int as *mut c_void },
                RenderParam { kind: PARAM_END, data: ptr::null_mut() },
            ];
            mpv_render_context_render(self.ctx, params.as_mut_ptr());

            // Put back what Slint had bound.
            let restore_fbo = NonZeroU32::new(prev_fbo as u32).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, restore_fbo);
            gl.bind_texture(glow::TEXTURE_2D, NonZeroU32::new(prev_tex as u32).map(glow::NativeTexture));
            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            // A fresh image every frame: Slint caches images it has drawn, so
            // handing back the same one would keep showing an old frame.
            Some(
                slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(target.0, [w, h].into())
                    .origin(slint::BorrowedOpenGLTextureOrigin::TopLeft)
                    .build(),
            )
        }
    }

    pub fn report_swap(&self) {
        unsafe { mpv_render_context_report_swap(self.ctx) };
    }
}

impl Drop for VideoOut {
    fn drop(&mut self) {
        // mpv requires the render context to go before the player core does.
        unsafe {
            mpv_render_context_free(self.ctx);
            for t in self.textures.iter_mut().filter_map(Option::take).chain(self.retired.drain(..)) {
                self.gl.delete_texture(t);
            }
            self.gl.delete_framebuffer(self.fbo);
        }
    }
}
