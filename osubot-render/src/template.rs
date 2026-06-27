use std::sync::OnceLock;
use tera::{Context, Tera};

fn tera_instance() -> &'static Tera {
    static TERA: OnceLock<Tera> = OnceLock::new();
    TERA.get_or_init(|| {
        let mut tera = Tera::default();
        tera.autoescape_on(vec!["html"]);
        tera.add_raw_template("_macros.html", include_str!("../templates/_macros.html"))
            .expect("failed to load _macros template");
        tera.add_raw_template("score.html", include_str!("../templates/score.html"))
            .expect("failed to load score template");
        tera.add_raw_template(
            "score_list.html",
            include_str!("../templates/score_list.html"),
        )
        .expect("failed to load score_list template");
        tera.add_raw_template("profile.html", include_str!("../templates/profile.html"))
            .expect("failed to load profile template");
        tera
    })
}

pub(crate) fn render(name: &str, ctx: &Context) -> String {
    tera_instance()
        .render(name, ctx)
        .map_err(|e| {
            tracing::error!(template = name, error = %e, "tera render failed");
            e
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tera_instance_loads() {
        let tera = tera_instance();
        assert!(tera.get_template("score.html").is_ok());
        assert!(tera.get_template("score_list.html").is_ok());
        assert!(tera.get_template("profile.html").is_ok());
        assert!(tera.get_template("_macros.html").is_ok());
    }
}
