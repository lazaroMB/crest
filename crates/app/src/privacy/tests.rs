use serde_json::json;

use super::*;
use crate::tools::budget::ResultBudget;

fn clean(text: &str) -> String {
    Redactor::default().redact(text).text.into_owned()
}

#[test]
fn masks_well_known_keys() {
    let cases = [
        ("AKIAIOSFODNN7EXAMPLE", "aws_key"),
        ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "github_token"),
        (
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
            "anthropic_key",
        ),
        ("sk-proj-abcdefghijklmnopqrstuvwxyz0123", "openai_key"),
        ("xoxb-1234567890-abcdefghij", "slack_token"),
        ("sk_live_abcdefghijklmnop1234", "stripe_key"),
        ("AIzaSyA1234567890abcdefghijklmnopqrstuv", "google_key"),
        (
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "jwt",
        ),
    ];
    for (secret, kind) in cases {
        assert_eq!(
            clean(&format!("value: {secret} end")),
            format!("value: [REDACTED:{kind}] end"),
            "{secret}"
        );
    }
}

#[test]
fn masks_private_keys_even_when_cut() {
    let pem = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\nb";
    assert_eq!(clean(pem), "a\n[REDACTED:private_key]\nb");
    let cut = "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk";
    assert_eq!(clean(cut), "a\n[REDACTED:private_key]");
}

#[test]
fn masks_assigned_secrets_but_not_code() {
    assert_eq!(
        clean("DB_PASSWORD=hunter22x"),
        "DB_PASSWORD=[REDACTED:secret]"
    );
    assert_eq!(
        clean(r#"{"api_key": "a1b2c3d4e5f6"}"#),
        r#"{"api_key": "[REDACTED:secret]"}"#
    );
    assert_eq!(
        clean("Authorization: Bearer abc123def456ghi789"),
        "Authorization: Bearer [REDACTED:token]"
    );
    assert_eq!(
        clean("postgres://admin:s3cr3t@db:5432/app"),
        "postgres://admin:[REDACTED:password]@db:5432/app"
    );
    for kept in [
        "password = ${DB_PASSWORD}",
        "let token = self.next_token();",
        "api_key: ANTHROPIC_API_KEY",
        "token: Option<Token>,",
        "password = changeme123",
        "max_tokens = 8000000000",
        "secret = <your-secret>",
        "sk-short",
        "https://user:$TOKEN@host",
    ] {
        assert_eq!(clean(kept), kept);
    }
}

#[test]
fn masks_personal_data_with_checks() {
    assert_eq!(
        clean("mail ana.lopez@acme.io now"),
        "mail [REDACTED:email] now"
    );
    assert_eq!(clean("card 4111 1111 1111 1111"), "card [REDACTED:card]");
    assert_eq!(
        clean("iban ES91 2100 0418 4502 0005 1332"),
        "iban [REDACTED:iban]"
    );
    assert_eq!(clean("dni 12345678Z"), "dni [REDACTED:national_id]");
    assert_eq!(clean("nie X1234567L"), "nie [REDACTED:national_id]");
    assert_eq!(clean("ssn 123-45-6789"), "ssn [REDACTED:national_id]");
    assert_eq!(clean("tel +34 600 123 456."), "tel [REDACTED:phone].");
    for kept in [
        "git@github.com:org/repo.git",
        "test@example.com",
        "49+noreply@users.noreply.github.com",
        "card 4111 1111 1111 1112",
        "ts 1697712345678",
        "iban ES91 2100 0418 4502 0005 1333",
        "dni 12345678A",
        "ssn 666-45-6789",
        "date 2026-10-19",
        "+    let x = 1;",
        "version +1.2",
    ] {
        assert_eq!(clean(kept), kept);
    }
}

#[test]
fn categories_allow_list_and_patterns_are_configurable() {
    let options = PrivacyOptions::from_json(&json!({
        "pii": { "email": false },
        "allow": ["4111 1111 1111 1111"],
        "patterns": [
            { "name": "ticket", "regex": "ACME-[0-9]{6}" },
            { "name": "pin", "regex": "pin=([0-9]{4})" },
        ],
    }))
    .unwrap();
    let redactor = Redactor::new(&options).unwrap();
    let text = "ana@acme.io 4111 1111 1111 1111 ACME-123456 pin=1234 AKIAIOSFODNN7EXAMPLE";
    assert_eq!(
        redactor.redact(text).text,
        "ana@acme.io 4111 1111 1111 1111 [REDACTED:ticket] pin=[REDACTED:pin] [REDACTED:aws_key]"
    );
    assert_eq!(
        Redactor::disabled().redact(text).text,
        text,
        "disabled masks nothing"
    );
}

#[test]
fn nothing_to_mask_borrows_the_text() {
    let redacted = Redactor::default().redact("fn main() {}");
    assert!(matches!(redacted.text, Cow::Borrowed(_)));
    assert_eq!(redacted.total(), 0);
}

#[test]
fn redacting_twice_changes_nothing() {
    let once = clean("token=a1b2c3d4e5f6 ana@acme.io");
    assert_eq!(clean(&once), once);
}

#[test]
fn tool_outputs_get_a_note_and_never_leak_a_cut_secret() {
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let text = format!("{} {secret}", "x".repeat(30));
    let output = Redactor::default().redact_output(ToolOutput::text(text));
    let output = ResultBudget::new(10).apply(output);
    let text = output.text_content();
    assert!(!text.contains("AKIA"), "{text}");
    let output = Redactor::default().redact_output(ToolOutput::text(format!("key {secret}")));
    assert_eq!(
        output.text_content(),
        "key [REDACTED:aws_key]\n[crest redacted 1 sensitive value: 1 aws_key; the [REDACTED:…] \
         placeholders are not the real content]"
    );
}

#[test]
fn options_reject_malformed_tables() {
    assert_eq!(
        PrivacyOptions::from_json(&json!([])),
        Ok(PrivacyOptions::default())
    );
    assert_eq!(
        PrivacyOptions::from_json(&json!(true)),
        Err(PrivacyConfigError::NotATable)
    );
    assert_eq!(
        PrivacyOptions::from_json(&json!({ "redact": true })),
        Err(PrivacyConfigError::UnknownField("redact".into()))
    );
    assert_eq!(
        PrivacyOptions::from_json(&json!({ "pii": { "dna": true } })),
        Err(PrivacyConfigError::UnknownCategory("dna".into()))
    );
    assert!(matches!(
        PrivacyOptions::from_json(&json!({ "patterns": [{ "name": "x", "regex": "(" }] })),
        Err(PrivacyConfigError::BadPattern { .. })
    ));
}

#[test]
fn a_workspace_can_only_tighten() {
    let global = json!({ "pii": { "email": false }, "allow": ["me@acme.io"] });
    let workspace = json!({
        "enabled": false,
        "prompts": false,
        "allow": ["AKIAIOSFODNN7EXAMPLE"],
        "pii": { "email": true, "card": false },
        "patterns": [{ "name": "ticket", "regex": "ACME-[0-9]+" }],
    });
    let merged = tighten(Some(&global), &workspace);
    assert_eq!(
        merged,
        json!({
            "pii": { "email": true },
            "allow": ["me@acme.io"],
            "patterns": [{ "name": "ticket", "regex": "ACME-[0-9]+" }],
        })
    );
    let options = PrivacyOptions::from_json(&merged).unwrap();
    assert!(options.enabled && options.prompts && options.pii.card);
}
