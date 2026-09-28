pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
