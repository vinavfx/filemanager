use crate::media::{Frame, Pixels};
use eframe::egui::{self, Rect, Vec2};
use eframe::glow::{self, HasContext};
use std::sync::{Arc, Mutex};

pub struct Renderer {
    program: glow::Program,
    vertex_array: glow::VertexArray,
    texture: glow::Texture,
    frame: Option<Arc<Frame>>,
    pub active: bool,
    pub error: Option<String>,
}

impl Renderer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = gl.create_program()?;
            for (kind, source) in [
                (glow::VERTEX_SHADER, include_str!("viewer.vert")),
                (glow::FRAGMENT_SHADER, include_str!("viewer.frag")),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let error = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return Err(error);
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let error = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(error);
            }
            let vertex_array = gl.create_vertex_array()?;
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Self {
                program,
                vertex_array,
                texture,
                frame: None,
                active: true,
                error: None,
            })
        }
    }

    fn draw(
        &mut self,
        gl: &glow::Context,
        frame: Arc<Frame>,
        origin: Vec2,
        size: Vec2,
        channel: i32,
        rotation: i32,
    ) -> Result<(), String> {
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if self
                .frame
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &frame))
            {
                let limit = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32;
                if frame.width > limit || frame.height > limit {
                    return Err(format!(
                        "Image exceeds the GPU texture limit ({limit} pixels)"
                    ));
                }
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                let (format, kind, data) = match &frame.pixels {
                    Pixels::Bytes(pixels) => (glow::RGBA8, glow::UNSIGNED_BYTE, pixels.as_slice()),
                    Pixels::Float(pixels) => {
                        (glow::RGBA32F, glow::FLOAT, bytemuck::cast_slice(pixels))
                    }
                };
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    format as i32,
                    frame.width as i32,
                    frame.height as i32,
                    0,
                    glow::RGBA,
                    kind,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
                let error = gl.get_error();
                if error != glow::NO_ERROR {
                    return Err(format!("OpenGL texture upload failed (0x{error:04x})"));
                }
                self.frame = Some(frame.clone());
            }
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.uniform_4_f32(
                gl.get_uniform_location(self.program, "image_rect").as_ref(),
                origin.x,
                origin.y,
                size.x,
                size.y,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "image_gamma")
                    .as_ref(),
                frame.gamma,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "smooth_preview")
                    .as_ref(),
                i32::from(frame.smooth_preview),
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "channel").as_ref(),
                channel,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "rotation").as_ref(),
                rotation,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "image_texture")
                    .as_ref(),
                0,
            );
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    pub fn paint(
        renderer: Arc<Mutex<Self>>,
        frame: Arc<Frame>,
        rect: Rect,
        origin: Vec2,
        size: Vec2,
        channel: i32,
        rotation: i32,
    ) -> egui::PaintCallback {
        let size = size / rect.size();
        let origin = origin / rect.size();
        let frame = Arc::downgrade(&frame);
        egui::PaintCallback {
            rect,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, painter| {
                if let Ok(mut renderer) = renderer.lock()
                    && renderer.active
                    && let Some(frame) = frame.upgrade()
                {
                    renderer.error = renderer
                        .draw(painter.gl(), frame, origin, size, channel, rotation)
                        .err();
                }
            })),
        }
    }

    pub fn clear(&mut self, gl: Option<&glow::Context>) {
        self.active = false;
        self.frame = None;
        self.error = None;
        if let Some(gl) = gl {
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    0,
                    0,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_texture(self.texture);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_program(self.program);
        }
        self.frame = None;
    }
}
