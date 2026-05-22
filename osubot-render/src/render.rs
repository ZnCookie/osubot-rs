use anyrender::{PaintScene as _, render_to_buffer};
use blitz::dom::DocumentConfig;
use blitz::dom::net::Resource;
use blitz::html::HtmlDocument;
use blitz::net::Provider;
use blitz::paint::paint_scene;
use blitz::traits::net::NetCallback;
use blitz::traits::shell::{ColorScheme, Viewport};
use parley::FontContext;
use peniko::Fill;
use peniko::kurbo::Rect;
use std::sync::{Arc, Mutex};

use crate::error::RenderError;

const GPU_MAX_DIM: u32 = 8192;

pub fn render_html_to_image(
    html: &str,
    font_ctx: &FontContext,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, u32, u32), RenderError> {
    let effective_scale = 1.0;
    let viewport_width = (width as f64 * effective_scale) as u32;
    let viewport_height = (height as f64 * effective_scale) as u32;

    let loaded_resources: Arc<Mutex<Vec<Resource>>> = Arc::new(Mutex::new(Vec::new()));
    let cb_resources = Arc::clone(&loaded_resources);
    let callback: Arc<dyn NetCallback<Resource>> = Arc::new(
        move |_doc_id: usize, result: Result<Resource, Option<String>>| {
            if let Ok(resource) = result {
                cb_resources.lock().unwrap().push(resource);
            }
        },
    );

    let net = Arc::new(Provider::new(callback));

    let mut document = HtmlDocument::from_html(
        html,
        DocumentConfig {
            base_url: None,
            net_provider: Some(Arc::clone(&net) as _),
            font_ctx: Some(font_ctx.clone()),
            viewport: Some(Viewport::new(
                viewport_width,
                viewport_height,
                effective_scale as f32,
                ColorScheme::Light,
            )),
            ..Default::default()
        },
    );

    loop {
        document.resolve(0.0);
        let resources: Vec<Resource> = loaded_resources.lock().unwrap().drain(..).collect();
        for resource in resources {
            document.load_resource(resource);
        }
        if net.is_empty() {
            break;
        }
    }

    document.resolve(0.0);

    let root = document.as_ref().root_element();
    let computed_height = root.final_layout.scroll_height().max(root.final_layout.size.height);
    let needed_logical_height = (computed_height as f64).max(height as f64);
    let render_width = (width as f64 * effective_scale) as u32;
    let total_physical_height = (needed_logical_height * effective_scale) as u32;

    if total_physical_height <= GPU_MAX_DIM {
        let buffer = render_to_buffer::<anyrender_vello::VelloImageRenderer, _>(
            |scene| {
                scene.fill(
                    Fill::NonZero,
                    Default::default(),
                    peniko::Color::WHITE,
                    Default::default(),
                    &Rect::new(0.0, 0.0, render_width as f64, total_physical_height as f64),
                );
                paint_scene(
                    scene,
                    document.as_ref(),
                    effective_scale,
                    render_width,
                    total_physical_height,
                );
            },
            render_width,
            total_physical_height,
        );
        Ok((buffer, render_width, total_physical_height))
    } else {
        let num_tiles =
            (total_physical_height as f64 / GPU_MAX_DIM as f64).ceil() as u32;
        let tile_logical_height = GPU_MAX_DIM as f64 / effective_scale;

        let mut all_pixels =
            Vec::with_capacity((render_width * total_physical_height * 4) as usize);

        for tile_idx in 0..num_tiles {
            let y_offset_css = tile_idx as f64 * tile_logical_height;
            let this_tile_phy_h = if tile_idx == num_tiles - 1 {
                total_physical_height - (tile_idx * GPU_MAX_DIM)
            } else {
                GPU_MAX_DIM
            };

            document.set_viewport_scroll(blitz::dom::Point {
                x: 0.0,
                y: y_offset_css,
            });

            let tile_buffer = render_to_buffer::<anyrender_vello::VelloImageRenderer, _>(
                |scene| {
                    scene.fill(
                        Fill::NonZero,
                        Default::default(),
                        peniko::Color::WHITE,
                        Default::default(),
                        &Rect::new(
                            0.0,
                            0.0,
                            render_width as f64,
                            this_tile_phy_h as f64,
                        ),
                    );
                    paint_scene(
                        scene,
                        document.as_ref(),
                        effective_scale,
                        render_width,
                        this_tile_phy_h,
                    );
                },
                render_width,
                this_tile_phy_h,
            );

            all_pixels.extend_from_slice(&tile_buffer);
        }

        document.set_viewport_scroll(blitz::dom::Point { x: 0.0, y: 0.0 });

        Ok((all_pixels, render_width, total_physical_height))
    }
}
