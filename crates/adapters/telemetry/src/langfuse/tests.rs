use std::collections::HashMap;

use super::*;

// Obviously fake values: no real credentials in tests.
const PUBLIC: &str = "public-test-value";
const SECRET: &str = "secret-test-value";

fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| vars.get(name).cloned()
}

fn config(host: &str) -> LangfuseConfig {
    LangfuseConfig {
        host: host.to_owned(),
        public_key: PUBLIC.to_owned(),
        secret_key: SECRET.to_owned(),
    }
}

#[test]
fn config_is_off_without_keys() {
    assert_eq!(LangfuseConfig::from_lookup(lookup(&[])).unwrap(), None);
    let blank = lookup(&[(PUBLIC_KEY_ENV, " "), (SECRET_KEY_ENV, "")]);
    assert_eq!(LangfuseConfig::from_lookup(blank).unwrap(), None);
}

#[test]
fn config_reads_keys_and_defaults_the_host() {
    let vars = lookup(&[(PUBLIC_KEY_ENV, PUBLIC), (SECRET_KEY_ENV, SECRET)]);
    let config = LangfuseConfig::from_lookup(vars).unwrap().unwrap();
    assert_eq!(config, self::config(DEFAULT_HOST));

    let vars = lookup(&[
        (HOST_ENV, "http://localhost:3000"),
        (PUBLIC_KEY_ENV, PUBLIC),
        (SECRET_KEY_ENV, SECRET),
    ]);
    let config = LangfuseConfig::from_lookup(vars).unwrap().unwrap();
    assert_eq!(config.host, "http://localhost:3000");
}

#[test]
fn config_with_one_key_is_an_error() {
    let err = LangfuseConfig::from_lookup(lookup(&[(PUBLIC_KEY_ENV, PUBLIC)])).unwrap_err();
    assert!(matches!(err, LangfuseError::MissingVar(SECRET_KEY_ENV)));
    let err = LangfuseConfig::from_lookup(lookup(&[(SECRET_KEY_ENV, SECRET)])).unwrap_err();
    assert!(matches!(err, LangfuseError::MissingVar(PUBLIC_KEY_ENV)));
}

#[test]
fn debug_hides_the_keys() {
    let debug = format!("{:?}", config("http://localhost:3000"));
    assert!(debug.contains("http://localhost:3000"));
    assert!(!debug.contains(PUBLIC), "{debug}");
    assert!(!debug.contains(SECRET), "{debug}");
}
