//! The warnings of a `print.prepare` result reach the agent in the language of
//! the app, with their codes as the command gives them.
//!
//! One test in this file: the language is process-wide state.

use dc_app::control::localise_print_warnings;
use dc_i18n::Lang;
use serde_json::{Value, json};

fn summary() -> Value {
    json!({
        "profile": "saturn-4-ultra-16k",
        "profile_validated": false,
        "warnings": [
            {
                "code": dc_print::profile::PROFILE_NOT_VALIDATED,
                "message": "the dimensional scale of printer profile saturn-4-ultra-16k",
            },
            { "code": "another-code", "message": "left as it is" },
        ],
        "plates": [],
    })
}

#[test]
fn the_warnings_of_the_prepared_plates_are_worded_in_the_language_of_the_app() {
    dc_app::register_bundles();

    dc_i18n::set_language(Lang::It).unwrap();
    let mut v = summary();
    localise_print_warnings(&mut v);
    let w = &v["warnings"];
    assert_eq!(w[0]["code"], dc_print::profile::PROFILE_NOT_VALIDATED);
    let it = w[0]["message"].as_str().unwrap();
    assert!(it.contains("saturn-4-ultra-16k"), "{it}");
    assert!(it.contains("non sono ancora stati verificati"), "{it}");
    // a warning the app has no sentence for stays as the command gave it
    assert_eq!(w[1], summary()["warnings"][1]);
    // nothing else of the result changes
    assert_eq!(v["plates"], json!([]));
    assert_eq!(v["profile_validated"], json!(false));

    dc_i18n::set_language(Lang::En).unwrap();
    let mut v = summary();
    localise_print_warnings(&mut v);
    let en = v["warnings"][0]["message"].as_str().unwrap();
    assert!(en.contains("are not yet checked"), "{en}");
    assert_ne!(en, it);

    // a result without warnings, or one that is not an object, is left alone
    let mut none = json!({ "plates": [] });
    localise_print_warnings(&mut none);
    assert_eq!(none, json!({ "plates": [] }));
    let mut other = json!([1, 2]);
    localise_print_warnings(&mut other);
    assert_eq!(other, json!([1, 2]));
}
