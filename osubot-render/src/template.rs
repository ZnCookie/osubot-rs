use std::sync::OnceLock;
use tera::{Context, Tera};

#[allow(dead_code)]
fn tera_instance() -> &'static Tera {
    static TERA: OnceLock<Tera> = OnceLock::new();
    TERA.get_or_init(|| {
        let mut tera = Tera::default();
        tera.add_raw_template("score", include_str!("../templates/score.html"))
            .expect("failed to load score template");
        tera.add_raw_template("score_list", include_str!("../templates/score_list.html"))
            .expect("failed to load score_list template");
        tera.add_raw_template("profile", include_str!("../templates/profile.html"))
            .expect("failed to load profile template");
        tera.add_raw_template("_macros", include_str!("../templates/_macros.html"))
            .expect("failed to load _macros template");
        tera
    })
}

#[allow(dead_code)]
pub(crate) fn render(name: &str, ctx: &Context) -> String {
    tera_instance()
        .render(name, ctx)
        .unwrap_or_else(|e| format!("Tera render error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tera_instance_loads() {
        let tera = tera_instance();
        assert!(tera.get_template("score").is_ok());
        assert!(tera.get_template("score_list").is_ok());
        assert!(tera.get_template("profile").is_ok());
        assert!(tera.get_template("_macros").is_ok());
    }
}
