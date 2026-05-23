use std::sync::OnceLock;
use takumi::resources::font::FontResource;
use takumi::GlobalContext;

static GLOBAL: OnceLock<GlobalContext> = OnceLock::new();

pub fn init() {
    let mut ctx = GlobalContext::default();

    let fonts: &[&[u8]] = &[
        include_bytes!("../fonts/NotoSans[wght].woff2"),
        include_bytes!("../fonts/NotoSansSC-Regular.ttf"),
        include_bytes!("../fonts/NotoSansTC-Regular.ttf"),
        include_bytes!("../fonts/NotoSansJP-Regular.ttf"),
        include_bytes!("../fonts/NotoSansKR-Regular.ttf"),
        include_bytes!("../fonts/NotoColorEmoji.ttf"),
    ];

    for data in fonts {
        match FontResource::new(*data).into_resolved() {
            Ok(resolved) => {
                let _ = ctx.font_context.load_and_store(resolved);
            }
            Err(e) => {
                tracing::warn!("failed to load font: {:?}", e);
            }
        }
    }

    if GLOBAL.set(ctx).is_err() {
        tracing::warn!("GlobalContext already initialized, skipping font load");
        return;
    }
    tracing::info!("fonts initialized ({} families)", fonts.len());
}

pub fn get() -> &'static GlobalContext {
    GLOBAL
        .get()
        .expect("font::init() must be called before any rendering")
}
