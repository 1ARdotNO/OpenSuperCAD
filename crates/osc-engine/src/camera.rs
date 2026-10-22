use serde::{Deserialize, Serialize};

use crate::BoundingBox;

/// OpenSCAD's default perspective field of view.
const FOV_DEGREES: f64 = 22.5;

/// The named view presets of OpenSCAD's *View* menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// OpenSCAD's default perspective (`--camera=…,55,0,25,…`).
    Iso,
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    /// A view from below-front, handy to inspect undersides and overhangs.
    Diagonal,
}

impl View {
    pub const ALL: [View; 8] = [
        View::Iso,
        View::Front,
        View::Back,
        View::Left,
        View::Right,
        View::Top,
        View::Bottom,
        View::Diagonal,
    ];

    /// Gimbal rotation `[x, y, z]` in degrees, as used by OpenSCAD.
    pub fn rotation(self) -> [f64; 3] {
        match self {
            View::Iso => [55.0, 0.0, 25.0],
            View::Front => [90.0, 0.0, 0.0],
            View::Back => [90.0, 0.0, 180.0],
            View::Left => [90.0, 0.0, 90.0],
            View::Right => [90.0, 0.0, 270.0],
            View::Top => [0.0, 0.0, 0.0],
            View::Bottom => [180.0, 0.0, 0.0],
            View::Diagonal => [125.0, 0.0, 35.0],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Iso => "iso",
            View::Front => "front",
            View::Back => "back",
            View::Left => "left",
            View::Right => "right",
            View::Top => "top",
            View::Bottom => "bottom",
            View::Diagonal => "diagonal",
        }
    }

    pub fn parse(name: &str) -> Option<View> {
        View::ALL
            .into_iter()
            .find(|v| v.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// A camera for snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Camera {
    /// A named preset; the model is auto-centred and framed.
    Preset { view: View },
    /// Gimbal camera: rotation in degrees around the (auto-centred) model.
    /// `distance` of `None` frames the whole model.
    Gimbal {
        rotation: [f64; 3],
        #[serde(default)]
        distance: Option<f64>,
        #[serde(default)]
        translate: [f64; 3],
    },
    /// Look-at camera in model coordinates.
    LookAt { eye: [f64; 3], center: [f64; 3] },
}

impl Camera {
    pub fn preset(view: View) -> Self {
        Camera::Preset { view }
    }

    /// The `openscad` command-line arguments selecting this camera.
    pub fn args(&self) -> Vec<String> {
        let fmt = |v: &[f64]| {
            v.iter()
                .map(|n| format!("{n}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        match self {
            Camera::Preset { view } => {
                let r = view.rotation();
                vec![
                    format!("--camera=0,0,0,{},0", fmt(&r)),
                    "--viewall".into(),
                    "--autocenter".into(),
                ]
            }
            Camera::Gimbal {
                rotation,
                distance,
                translate,
            } => {
                let mut args = vec![format!(
                    "--camera={},{},{}",
                    fmt(translate),
                    fmt(rotation),
                    distance.unwrap_or(0.0)
                )];
                if distance.is_none() {
                    args.push("--viewall".into());
                    args.push("--autocenter".into());
                }
                args
            }
            Camera::LookAt { eye, center } => {
                vec![format!("--camera={},{}", fmt(eye), fmt(center))]
            }
        }
    }

    /// Whether OpenSCAD picks the framing (`--viewall --autocenter`).
    pub fn is_auto_framed(&self) -> bool {
        matches!(
            self,
            Camera::Preset { .. } | Camera::Gimbal { distance: None, .. }
        )
    }

    /// An auto-framed camera as an explicit one fitting `bbox` the way
    /// OpenSCAD's "view all" does; other cameras are returned unchanged.
    pub fn framed(&self, bbox: &BoundingBox) -> Camera {
        let rotation = match self {
            Camera::Preset { view } => view.rotation(),
            Camera::Gimbal {
                rotation,
                distance: None,
                ..
            } => *rotation,
            _ => return self.clone(),
        };
        Camera::Gimbal {
            rotation,
            distance: Some(bbox.radius() / (FOV_DEGREES / 2.0).to_radians().sin()),
            translate: bbox.center(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Camera::Preset { view } => view.name().to_owned(),
            Camera::Gimbal { rotation, .. } => {
                format!("rot_{}_{}_{}", rotation[0], rotation[1], rotation[2])
            }
            Camera::LookAt { .. } => "lookat".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_args() {
        assert_eq!(
            Camera::preset(View::Iso).args(),
            ["--camera=0,0,0,55,0,25,0", "--viewall", "--autocenter"]
        );
        assert_eq!(View::parse(" Front"), Some(View::Front));
    }

    #[test]
    fn camera_json() {
        let c: Camera =
            serde_json::from_str(r#"{"type":"gimbal","rotation":[10,0,45],"distance":200}"#)
                .unwrap();
        assert_eq!(c.args(), ["--camera=0,0,0,10,0,45,200"]);
        let c: Camera = serde_json::from_str(r#"{"type":"preset","view":"top"}"#).unwrap();
        assert_eq!(c, Camera::preset(View::Top));
    }

    #[test]
    fn framing_fits_the_box() {
        let bbox = BoundingBox {
            min: [3.0, 4.0, 5.0],
            max: [13.0, 10.0, 9.0],
        };
        let Camera::Gimbal {
            rotation,
            distance: Some(d),
            translate,
        } = Camera::preset(View::Iso).framed(&bbox)
        else {
            panic!("expected an explicit gimbal camera");
        };
        assert_eq!((rotation, translate), ([55.0, 0.0, 25.0], [8.0, 7.0, 7.0]));
        assert!((d - 31.5977).abs() < 1e-3, "{d}");
        let look = Camera::LookAt {
            eye: [1.0; 3],
            center: [0.0; 3],
        };
        assert_eq!(look.framed(&bbox), look);
        assert!(!look.is_auto_framed());
    }
}
