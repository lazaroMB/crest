//! Body of the forecast card: the series, its decomposition into trend,
//! seasonal components and remainder, the forecast with 80% and 95%
//! intervals, and the accuracy on a held-out tail.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// One seasonal component of a decomposition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SeasonalComponent {
    /// Its period, in observations.
    pub period: usize,
    /// One value per observation.
    pub values: Vec<f64>,
}

/// The forecast of the held-out tail by a model fitted without it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Holdout {
    /// Index of the first held-out observation.
    pub start: usize,
    /// The forecast of the held-out observations.
    pub predicted: Vec<f64>,
}

/// Body of a forecast card. The forecast continues the series' indices
/// at `values.len()`, `values.len() + 1`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForecastBody {
    /// The variable.
    pub variable: String,
    /// Time labels of the observations, when the data had a time column.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time: Vec<String>,
    /// The model, e.g. `MSTL (periods 12) + AutoETS trend`.
    pub model: String,
    /// The observations.
    pub values: Vec<f64>,
    /// Trend of the decomposition (empty without seasonal periods).
    #[serde(default)]
    pub trend: Vec<f64>,
    /// Seasonal components, one per period.
    #[serde(default)]
    pub seasonal: Vec<SeasonalComponent>,
    /// Remainder of the decomposition.
    #[serde(default)]
    pub remainder: Vec<f64>,
    /// Point forecasts.
    pub forecast: Vec<f64>,
    /// 80% interval, lower bounds.
    pub lo80: Vec<f64>,
    /// 80% interval, upper bounds.
    pub hi80: Vec<f64>,
    /// 95% interval, lower bounds.
    pub lo95: Vec<f64>,
    /// 95% interval, upper bounds.
    pub hi95: Vec<f64>,
    /// The hold-out check, when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout: Option<Holdout>,
}

impl ForecastBody {
    /// Checks that the components match the series and the intervals the
    /// forecast.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for the first mismatch.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let n = self.values.len();
        let h = self.forecast.len();
        let component = |len: usize| len == 0 || len == n;
        let ok = component(self.trend.len())
            && component(self.remainder.len())
            && self.seasonal.iter().all(|s| s.values.len() == n)
            && [&self.lo80, &self.hi80, &self.lo95, &self.hi95]
                .iter()
                .all(|v| v.len() == h)
            && (self.time.is_empty() || self.time.len() == n)
            && self
                .holdout
                .as_ref()
                .is_none_or(|ho| ho.start + ho.predicted.len() == n);
        if ok {
            Ok(())
        } else {
            Err(ComputedError::Body(
                "forecast components and intervals must match the series".into(),
            ))
        }
    }
}
