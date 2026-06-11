use anyrender::{render_to_buffer, PaintScene as _};
use blitz::dom::net::Resource;
use blitz::dom::DocumentConfig;
use blitz::html::HtmlDocument;
use blitz::net::Provider;
use blitz::paint::paint_scene;
use blitz::traits::net::NetCallback;
use blitz::traits::shell::{ColorScheme, Viewport};
use parley::FontContext;
use peniko::kurbo::Rect;
use peniko::Fill;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::RenderError;

const MAX_TILE_HEIGHT: u32 = 8192;
const MAX_RESOURCE_ITERATIONS: u32 = 50;

pub fn render_html_to_image(
    html: &str,
    font_ctx: &FontContext,
    width: u32,
    height: u32,
    cancelled: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), RenderError> {
    let viewport_width = width;
    let viewport_height = height;

    let (resource_tx, resource_rx) = mpsc::channel::<()>();

    let loaded_resources: Arc<Mutex<Vec<Resource>>> = Arc::new(Mutex::new(Vec::new()));
    let cb_resources = Arc::clone(&loaded_resources);
    let callback: Arc<dyn NetCallback<Resource>> = Arc::new(
        move |_doc_id: usize, result: Result<Resource, Option<String>>| {
            if let Ok(resource) = result {
                cb_resources
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(resource);
                let _ = resource_tx.send(());
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
                1.0f32,
                ColorScheme::Light,
            )),
            ..Default::default()
        },
    );

    for _ in 0..MAX_RESOURCE_ITERATIONS {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RenderError::Render("render cancelled".into()));
        }
        document.resolve(0.0);
        let resources: Vec<Resource> = loaded_resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for resource in resources {
            document.load_resource(resource);
        }
        if net.is_empty() {
            break;
        }
        match resource_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!("resource channel disconnected, proceeding with partial resources");
                break;
            }
        }
    }

    document.resolve(0.0);

    let root = document.as_ref().root_element();
    let computed_height = root
        .final_layout
        .scroll_height()
        .max(root.final_layout.size.height);
    let needed_logical_height = (computed_height as f64).max(height as f64);
    let render_width = width;
    let total_physical_height = needed_logical_height as u32;

    if cancelled.load(Ordering::Relaxed) {
        return Err(RenderError::Render("render cancelled".into()));
    }
    if total_physical_height <= MAX_TILE_HEIGHT {
        let buffer = render_to_buffer::<anyrender_vello_cpu::VelloCpuImageRenderer, _>(
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
                    1.0,
                    render_width,
                    total_physical_height,
                );
            },
            render_width,
            total_physical_height,
        );
        let expected = (render_width as usize)
            .saturating_mul(total_physical_height as usize)
            .saturating_mul(4);
        if buffer.len() != expected {
            return Err(RenderError::Render(format!(
                "non-tiled render size mismatch: expected {}, got {}",
                expected,
                buffer.len()
            )));
        }
        Ok((buffer, render_width, total_physical_height))
    } else {
        let num_tiles = (total_physical_height as f64 / MAX_TILE_HEIGHT as f64).ceil() as u32;
        let tile_logical_height = MAX_TILE_HEIGHT as f64;

        let mut all_pixels = Vec::with_capacity(
            (render_width as usize)
                .saturating_mul(total_physical_height as usize)
                .saturating_mul(4),
        );

        for tile_idx in 0..num_tiles {
            if cancelled.load(Ordering::Relaxed) {
                return Err(RenderError::Render("render cancelled".into()));
            }
            let y_offset_css = tile_idx as f64 * tile_logical_height;
            let this_tile_phy_h = if tile_idx == num_tiles - 1 {
                total_physical_height - (tile_idx * MAX_TILE_HEIGHT)
            } else {
                MAX_TILE_HEIGHT
            };

            document.set_viewport_scroll(blitz::dom::Point {
                x: 0.0,
                y: y_offset_css,
            });

            let tile_buffer = render_to_buffer::<anyrender_vello_cpu::VelloCpuImageRenderer, _>(
                |scene| {
                    scene.fill(
                        Fill::NonZero,
                        Default::default(),
                        peniko::Color::WHITE,
                        Default::default(),
                        &Rect::new(0.0, 0.0, render_width as f64, this_tile_phy_h as f64),
                    );
                    paint_scene(scene, document.as_ref(), 1.0, render_width, this_tile_phy_h);
                },
                render_width,
                this_tile_phy_h,
            );

            let expected_tile_size = (render_width as usize)
                .saturating_mul(this_tile_phy_h as usize)
                .saturating_mul(4);
            if tile_buffer.len() != expected_tile_size {
                return Err(RenderError::Render(format!(
                    "tile {} size mismatch: expected {}, got {}",
                    tile_idx,
                    expected_tile_size,
                    tile_buffer.len()
                )));
            }
            all_pixels.extend_from_slice(&tile_buffer);
        }

        document.set_viewport_scroll(blitz::dom::Point { x: 0.0, y: 0.0 });

        Ok((all_pixels, render_width, total_physical_height))
    }
}
