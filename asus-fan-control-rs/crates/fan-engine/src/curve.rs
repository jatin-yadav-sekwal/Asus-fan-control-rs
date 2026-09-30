use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp_c: f32,
    pub fan_percent: f32,
}

impl CurvePoint {
    pub fn new(temp_c: f32, fan_percent: f32) -> Self {
        Self {
            temp_c,
            fan_percent: fan_percent.clamp(0.0, 100.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanCurve {
    pub name: String,
    pub points: Vec<CurvePoint>,
}

impl FanCurve {
    pub fn new(name: impl Into<String>, mut points: Vec<CurvePoint>) -> Self {
        points.sort_by(|a, b| a.temp_c.partial_cmp(&b.temp_c).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            name: name.into(),
            points,
        }
    }

    /// Evaluates target fan percentage for a given temperature via linear interpolation (Lerp).
    pub fn evaluate(&self, temp_c: f32) -> f32 {
        if self.points.is_empty() {
            return 0.0;
        }

        if self.points.len() == 1 {
            return self.points[0].fan_percent;
        }

        // Clamp below lowest point
        if temp_c <= self.points[0].temp_c {
            return self.points[0].fan_percent;
        }

        // Clamp above highest point
        let last_idx = self.points.len() - 1;
        if temp_c >= self.points[last_idx].temp_c {
            return self.points[last_idx].fan_percent;
        }

        // Find bounding points
        for i in 0..last_idx {
            let p0 = &self.points[i];
            let p1 = &self.points[i + 1];

            if temp_c >= p0.temp_c && temp_c <= p1.temp_c {
                let range = p1.temp_c - p0.temp_c;
                if range <= 0.001 {
                    return p0.fan_percent;
                }
                let t = (temp_c - p0.temp_c) / range;
                let interp = p0.fan_percent + t * (p1.fan_percent - p0.fan_percent);
                return interp.clamp(0.0, 100.0);
            }
        }

        self.points[last_idx].fan_percent
    }

    /// Quiet profile: fans stay near inaudible below 50°C, ramping gently under sustained load.
    pub fn silent() -> Self {
        Self::new(
            "Silent",
            vec![
                CurvePoint::new(40.0, 0.0),
                CurvePoint::new(55.0, 25.0),
                CurvePoint::new(70.0, 45.0),
                CurvePoint::new(82.0, 70.0),
                CurvePoint::new(90.0, 100.0),
            ],
        )
    }

    /// Balanced profile: good compromise between acoustics and thermal performance.
    pub fn balanced() -> Self {
        Self::new(
            "Balanced",
            vec![
                CurvePoint::new(35.0, 20.0),
                CurvePoint::new(50.0, 35.0),
                CurvePoint::new(65.0, 55.0),
                CurvePoint::new(78.0, 80.0),
                CurvePoint::new(88.0, 100.0),
            ],
        )
    }

    /// Turbo profile: aggressive fan curve for heavy gaming or sustained rendering.
    pub fn turbo() -> Self {
        Self::new(
            "Turbo",
            vec![
                CurvePoint::new(35.0, 35.0),
                CurvePoint::new(50.0, 60.0),
                CurvePoint::new(65.0, 85.0),
                CurvePoint::new(78.0, 100.0),
            ],
        )
    }

    /// Fixed 100% full speed curve.
    pub fn full_speed() -> Self {
        Self::constant(100)
    }

    /// Constant manual percentage curve.
    pub fn constant(percent: u8) -> Self {
        let p = percent.clamp(0, 100) as f32;
        Self::new(
            format!("Manual ({}%)", percent),
            vec![
                CurvePoint::new(0.0, p),
                CurvePoint::new(100.0, p),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_curve_interpolation() {
        let curve = FanCurve::new(
            "Test",
            vec![
                CurvePoint::new(40.0, 20.0),
                CurvePoint::new(60.0, 60.0),
            ],
        );

        assert_eq!(curve.evaluate(30.0), 20.0); // Clamp below
        assert_eq!(curve.evaluate(40.0), 20.0);
        assert_eq!(curve.evaluate(50.0), 40.0); // Midpoint Lerp
        assert_eq!(curve.evaluate(60.0), 60.0);
        assert_eq!(curve.evaluate(75.0), 60.0); // Clamp above
    }
}
