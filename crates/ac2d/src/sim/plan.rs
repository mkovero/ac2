//! An ac2 plan as the simulator's room: the outline extruded to the ceiling, a material per
//! surface, the speakers as omnidirectional sources and the receivers as points.

use ac2_proto::model::{
    DEFAULT_FLOOR_MATERIAL, DEFAULT_WALL_MATERIAL, MarkerKind, PlanSpec, SimRefusal, Surface,
};
use ac2_roomsim::{Air, Material, MaterialId, Prism, Room, SurfaceId, Surfaces};

/// The plan as a room: validated as `plan.save` validates it, refused outdoors, every tag's
/// material looked up by name (an untagged wall or ceiling is [`DEFAULT_WALL_MATERIAL`], an
/// untagged floor [`DEFAULT_FLOOR_MATERIAL`]). The air is the simulator's default
/// (20 °C, 50 % RH, 101.325 kPa): a plan does not say what the air is.
pub(crate) fn prism(plan: &PlanSpec) -> Result<Prism, SimRefusal> {
    ac2_room::plan::validate(plan).map_err(|e| SimRefusal::Plan {
        reason: e.to_string(),
    })?;
    if plan.ceiling.is_none() {
        return Err(SimRefusal::Outdoor);
    }
    let named = |surface: Surface, default: &str| -> Result<Material, SimRefusal> {
        let name = plan
            .surfaces
            .iter()
            .find(|t| t.surface == surface)
            .map_or(default, |t| t.material.as_str());
        MaterialId::from_name(name)
            .map(Material::from)
            .ok_or_else(|| SimRefusal::UnknownMaterial {
                surface,
                name: name.to_owned(),
            })
    };
    let walls = (0..plan.outline.len())
        .map(|edge| {
            let edge = u32::try_from(edge).unwrap_or(u32::MAX);
            named(Surface::Wall { edge }, DEFAULT_WALL_MATERIAL)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let room = Room {
        outline: plan.outline.clone(),
        height: plan.ceiling,
        surfaces: Surfaces {
            walls,
            floor: named(Surface::Floor, DEFAULT_FLOOR_MATERIAL)?,
            ceiling: named(Surface::Ceiling, DEFAULT_WALL_MATERIAL)?,
        },
        air: Air::default(),
    };
    Prism::new(&room).map_err(|e| SimRefusal::Plan {
        reason: e.to_string(),
    })
}

/// The speaker marker `name`'s position, checked strictly inside the room.
pub(crate) fn speaker(plan: &PlanSpec, prism: &Prism, name: &str) -> Result<[f64; 3], SimRefusal> {
    let m = plan
        .markers
        .iter()
        .find(|m| m.kind == MarkerKind::Speaker && m.name == name)
        .ok_or_else(|| SimRefusal::NoSpeaker {
            name: name.to_owned(),
        })?;
    inside(prism, &m.name, m.at)
}

/// `at`, when strictly inside the room; `name` names it in the refusal.
pub(crate) fn inside(prism: &Prism, name: &str, at: [f64; 3]) -> Result<[f64; 3], SimRefusal> {
    if prism.contains(at) {
        Ok(at)
    } else {
        Err(SimRefusal::Outside {
            name: name.to_owned(),
        })
    }
}

/// The plan's name for a surface of the simulator's room.
pub(crate) fn surface(s: SurfaceId) -> Surface {
    match s {
        SurfaceId::Wall { edge } => Surface::Wall { edge },
        SurfaceId::Floor => Surface::Floor,
        SurfaceId::Ceiling => Surface::Ceiling,
    }
}
