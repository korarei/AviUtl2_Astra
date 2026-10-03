use crate::http;
use serde_json::Value;

pub(crate) fn get(id: &str) -> anyhow::Result<Value> {
    const CATALOG_URL: &str = "https://raw.githubusercontent.com/Neosku/aviutl2-catalog-data/main/index.json";
    const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;

    tracing::info!("Requesting AviUtl2 Catalog");

    let bytes = http::read(
        http::client()?.get(CATALOG_URL).send()?.error_for_status()?,
        CATALOG_URL,
        MAX_CATALOG_BYTES,
    )?;

    serde_json::from_slice::<Vec<Value>>(&bytes)?
        .into_iter()
        .find(|package| {
            package
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|package_id| package_id == id)
        })
        .ok_or_else(|| anyhow::anyhow!("package '{id}' not found in catalog"))
}
