/// Geographic position and kinematics of a simulation node.
///
/// Used by the mobility manager to expose node locations via the HTTP API.
/// Coordinates are WGS84. Speed is in m/s. Bearing is in degrees clockwise
/// from north (0 = north, 90 = east).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct NodePosition {
    pub lat: f64,
    pub lon: f64,
    /// Current speed in m/s.
    pub speed: f64,
    /// Bearing in degrees clockwise from north.
    pub bearing: f64,
}
