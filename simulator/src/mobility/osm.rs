//! OpenStreetMap data fetching and parsing.
//!
//! Fetches drivable road network data from the Overpass API for a given
//! bounding box and caches the raw JSON response to disk so repeated restarts
//! don't re-fetch.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Raw OSM node (intersection or road shape point).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsmNode {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
}

/// Raw OSM way (road segment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsmWay {
    pub id: i64,
    pub node_ids: Vec<i64>,
    pub oneway: bool,
}

/// Bounding box for OSM queries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

/// Cached OSM dataset written/read from disk.
#[derive(Serialize, Deserialize)]
struct OsmCache {
    nodes: Vec<OsmNode>,
    ways: Vec<OsmWay>,
}

/// Fetch OSM road network for `bbox`.
///
/// If `cache_path` exists on disk the data is loaded from there; otherwise the
/// Overpass API is queried and the result is written to `cache_path`.
pub async fn fetch_osm(bbox: BoundingBox, cache_path: &str) -> Result<(Vec<OsmNode>, Vec<OsmWay>)> {
    let cache = Path::new(cache_path);

    if cache.exists() {
        tracing::info!(path = cache_path, "Loading OSM data from cache");
        let raw = tokio::fs::read_to_string(cache)
            .await
            .with_context(|| format!("reading OSM cache {cache_path}"))?;
        let cached: OsmCache =
            serde_json::from_str(&raw).with_context(|| "parsing OSM cache JSON")?;
        tracing::info!(
            nodes = cached.nodes.len(),
            ways = cached.ways.len(),
            "OSM cache loaded"
        );
        return Ok((cached.nodes, cached.ways));
    }

    tracing::info!(?bbox, "Fetching OSM data from Overpass API");
    let (nodes, ways) = query_overpass(bbox).await?;

    let cached = OsmCache {
        nodes: nodes.clone(),
        ways: ways.clone(),
    };
    let json = serde_json::to_string_pretty(&cached).context("serialising OSM cache")?;
    tokio::fs::write(cache, json)
        .await
        .with_context(|| format!("writing OSM cache to {cache_path}"))?;
    tracing::info!(path = cache_path, "OSM data cached to disk");

    Ok((nodes, ways))
}

/// Raw Overpass JSON element deserialization helpers.
#[derive(Deserialize)]
struct OverpassResponse {
    elements: Vec<OverpassElement>,
}

#[derive(Deserialize)]
struct OverpassElement {
    #[serde(rename = "type")]
    element_type: String,
    id: i64,
    // Present on nodes
    lat: Option<f64>,
    lon: Option<f64>,
    // Present on ways
    nodes: Option<Vec<i64>>,
    tags: Option<serde_json::Value>,
}

const OVERPASS_MIRRORS: &[&str] = &[
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass.openstreetmap.ru/api/interpreter",
];

async fn query_overpass(bbox: BoundingBox) -> Result<(Vec<OsmNode>, Vec<OsmWay>)> {
    let query = format!(
        r#"[out:json][timeout:60];
(
  way["highway"]["highway"!~"footway|cycleway|path|pedestrian|service|track"]["access"!~"private|no"]["indoor"!="yes"]["tunnel"!="building_passage"]({min_lat},{min_lon},{max_lat},{max_lon});
);
(._;>;);
out body;"#,
        min_lat = bbox.min_lat,
        min_lon = bbox.min_lon,
        max_lat = bbox.max_lat,
        max_lon = bbox.max_lon,
    );

    tracing::debug!(query, "Overpass query");

    let client = reqwest::Client::builder()
        .user_agent("vigilant-parakeet/simulator (OSM mobility)")
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .context("building HTTP client")?;

    let mut last_err = anyhow::anyhow!("no Overpass mirrors configured");
    for &mirror in OVERPASS_MIRRORS {
        tracing::info!(mirror, "Trying Overpass mirror");
        let result = client.post(mirror).form(&[("data", &query)]).send().await;

        let response = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(mirror, error = %e, "Overpass mirror unreachable, trying next");
                last_err = anyhow::anyhow!("{mirror}: {e}");
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(mirror, %status, body, "Overpass mirror returned error, trying next");
            last_err = anyhow::anyhow!("{mirror}: HTTP {status}: {body}");
            continue;
        }

        // Success — parse and return
        let overpass: OverpassResponse = response
            .json()
            .await
            .context("parsing Overpass API response")?;

        tracing::info!(
            mirror,
            elements = overpass.elements.len(),
            "Overpass API response received"
        );

        let mut nodes = Vec::new();
        let mut ways = Vec::new();

        for elem in overpass.elements {
            match elem.element_type.as_str() {
                "node" => {
                    if let (Some(lat), Some(lon)) = (elem.lat, elem.lon) {
                        nodes.push(OsmNode {
                            id: elem.id,
                            lat,
                            lon,
                        });
                    }
                }
                "way" => {
                    if let Some(node_ids) = elem.nodes {
                        if node_ids.len() >= 2 {
                            let oneway = elem
                                .tags
                                .as_ref()
                                .and_then(|t| t.get("oneway"))
                                .and_then(|v| v.as_str())
                                .map(|s| s == "yes" || s == "1" || s == "true")
                                .unwrap_or(false);
                            ways.push(OsmWay {
                                id: elem.id,
                                node_ids,
                                oneway,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        tracing::info!(nodes = nodes.len(), ways = ways.len(), "OSM data parsed");
        return Ok((nodes, ways));
    }

    anyhow::bail!(
        "All Overpass mirrors failed — check internet connectivity or place osm_cache.json \
         manually. Last error: {last_err}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box_serialization() {
        let bbox = BoundingBox {
            min_lat: 41.145,
            max_lat: 41.165,
            min_lon: -8.630,
            max_lon: -8.610,
        };
        let json = serde_json::to_string(&bbox).unwrap();
        let back: BoundingBox = serde_json::from_str(&json).unwrap();
        assert!((back.min_lat - bbox.min_lat).abs() < 1e-9);
        assert!((back.max_lon - bbox.max_lon).abs() < 1e-9);
    }
}
