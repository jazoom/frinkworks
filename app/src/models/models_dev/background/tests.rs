use super::super::catalogue::ModelLimit;
use super::*;

fn model(id: &str, date: &str, input: f64) -> Model {
    Model {
        id: id.to_owned(),
        reasoning: false,
        deprecated: false,
        efforts: Vec::new(),
        attachment: false,
        image_input: false,
        supports_tools: true,
        limit: ModelLimit {
            context: 4096,
            output: 128,
        },
        background: Some(BackgroundMetadata {
            release_date: date.to_owned(),
            input_cost: input,
            output_cost: 0.5,
            output_limit: 128,
        }),
        prices: None,
    }
}

#[test]
fn selection_enforces_age_band_and_ceiling() {
    let today = Date::from_calendar_date(2026, Month::September, 7).unwrap();
    let models = vec![
        model("old", "2025-09-07", 0.1),
        model("recent", "2026-08-07", 0.2),
        model("outside-band", "2026-09-01", 0.4),
        model("flagship", "2026-09-02", 2.0),
        model("future", "2026-10-01", 0.01),
        model("expired", "2025-09-06", 0.01),
    ];
    assert_eq!(select(&models, today).unwrap().id, "recent");
    assert!(select(&models[3..], today).is_none());
}

#[test]
fn untrusted_dates_prices_and_limits_fail_closed() {
    for date in ["", "2026-02-30", "2026-13", "2026-9-01", "2026-09-01-extra"] {
        assert!(release_date(date).is_none());
    }
    assert_eq!(release_date("2026-09").unwrap().day(), 1);
    for price in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            !model("bad", "2026-09-01", price)
                .background
                .unwrap()
                .valid()
        );
    }
    let today = release_date("2026-09-07").unwrap();
    let mut missing = model("unknown", "2026-09-01", 0.1);
    missing.background = None;
    assert!(select(&[missing], today).is_none());
    for id in [
        "some:free",
        "guard-model",
        "model-preview",
        "realtime-model",
    ] {
        assert!(unsuitable(id));
    }
}
