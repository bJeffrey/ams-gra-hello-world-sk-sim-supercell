//! Generation of SuperCell inputs from the versioned ai-bm-sim scenario contract.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Generated SuperCell configuration filename.
pub const GENERATED_TOML_NAME: &str = "ai_bm_sim_ecosystem.toml";

#[derive(Debug, Deserialize)]
struct Document {
    scenario: Scenario,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    sites: Vec<Site>,
    policy_compatibility: Compatibility,
}

#[derive(Debug, Deserialize)]
struct Site {
    instance_id: String,
    maneuvers: Maneuvers,
}

#[derive(Debug, Deserialize)]
struct Maneuvers {
    location: String,
}

#[derive(Debug, Deserialize)]
struct Compatibility {
    schema_version: String,
    bma_training_preset: String,
    bma_policy_artifact: String,
    initial_conditions: InitialConditions,
}

#[derive(Debug, Deserialize)]
struct InitialConditions {
    blue_cruise_speed_kts: f64,
    blue_heading_deg_true: f64,
    red_speed_kts: f64,
    red_heading_deg_true: f64,
    red_motion_mode: String,
    blue: Vec<PolicyPlatform>,
    red: PolicyPlatform,
}

#[derive(Debug, Deserialize)]
struct PolicyPlatform {
    instance_id: String,
    grid_xy: [f64; 2],
}

#[derive(Clone, Copy, Debug)]
struct Lla {
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
}

struct Platform {
    name: &'static str,
    entity_id: u16,
    force_id: u8,
    aircraft: &'static str,
    control_port: u16,
    reset_name: &'static str,
    location: Lla,
    heading_deg: f64,
    speed_kts: f64,
}

/// Generate a SuperCell TOML plus five JSBSim reset and run-script files.
///
/// The supplied template remains authoritative for SuperCell transport, timing,
/// and CAL settings. Only its legacy entity fixture is replaced.
pub fn generate(scenario_path: &Path, template_path: &Path, output_dir: &Path) -> Result<()> {
    let document: Document = serde_yaml::from_str(
        &fs::read_to_string(scenario_path)
            .with_context(|| format!("read scenario contract: {}", scenario_path.display()))?,
    )
    .with_context(|| format!("parse scenario contract: {}", scenario_path.display()))?;
    validate(&document.scenario)?;
    let platforms = platforms(&document.scenario)?;
    let mut template: toml::Value = toml::from_str(
        &fs::read_to_string(template_path)
            .with_context(|| format!("read SuperCell template: {}", template_path.display()))?,
    )
    .with_context(|| format!("parse SuperCell template: {}", template_path.display()))?;
    let entities: toml::Value =
        toml::from_str(&entities_toml(&platforms)).context("build generated entities")?;
    template
        .as_table_mut()
        .context("template must be a TOML table")?
        .insert(
            "entities".to_owned(),
            entities
                .get("entities")
                .cloned()
                .context("generated entities missing root table")?,
        );
    fs::create_dir_all(output_dir.join("jsbsim_initial_conditions"))?;
    fs::create_dir_all(output_dir.join("jsbsim_scripts"))?;
    let compatibility = &document.scenario.policy_compatibility;
    let header = format!(
        "# Generated from {}.\n# scenario={} schema={} preset={} policy={} red_motion={}\n# Do not edit: regenerate from the ai-bm-sim contract.\n\n",
        scenario_path.display(),
        document.scenario.name,
        compatibility.schema_version,
        compatibility.bma_training_preset,
        compatibility.bma_policy_artifact,
        compatibility.initial_conditions.red_motion_mode
    );
    fs::write(
        output_dir.join(GENERATED_TOML_NAME),
        format!("{header}{}", toml::to_string_pretty(&template)?),
    )?;
    for platform in &platforms {
        fs::write(
            output_dir
                .join("jsbsim_initial_conditions")
                .join(format!("{}_reset00.xml", platform.reset_name)),
            reset_xml(platform),
        )?;
        fs::write(
            output_dir
                .join("jsbsim_scripts")
                .join(format!("{}_run.xml", platform.reset_name)),
            runscript_xml(platform),
        )?;
    }
    Ok(())
}

fn validate(scenario: &Scenario) -> Result<()> {
    let initial = &scenario.policy_compatibility.initial_conditions;
    if scenario.policy_compatibility.schema_version != "1.0" {
        bail!("unsupported policy_compatibility schema");
    }
    if initial.blue.len() != 4 {
        bail!("SuperCell policy scenario requires exactly four blue platforms");
    }
    if initial.blue_cruise_speed_kts <= 0.0 || initial.red_speed_kts <= 0.0 {
        bail!("policy speeds must be positive");
    }
    for platform in initial.blue.iter().chain(std::iter::once(&initial.red)) {
        if !scenario
            .sites
            .iter()
            .any(|site| site.instance_id == platform.instance_id)
        {
            bail!("missing scenario site for {}", platform.instance_id);
        }
        if !platform
            .grid_xy
            .iter()
            .all(|value| (0.0..=20.0).contains(value))
        {
            bail!("{} is outside the 21x21 BMA grid", platform.instance_id);
        }
    }
    Ok(())
}

fn platforms(scenario: &Scenario) -> Result<Vec<Platform>> {
    let initial = &scenario.policy_compatibility.initial_conditions;
    let bindings = [
        ("Blue-1", 1, "f16", 21110, "ownship"),
        ("Blue-2", 2, "f16", 21110, "blue_2"),
        ("Blue-3", 3, "f16", 21110, "blue_3"),
        ("Blue-4", 4, "f16", 21110, "blue_4"),
    ];
    let mut result = Vec::with_capacity(5);
    for (policy, (name, entity_id, aircraft, control_port, reset_name)) in
        initial.blue.iter().zip(bindings)
    {
        result.push(Platform {
            name,
            entity_id,
            force_id: 1,
            aircraft,
            control_port,
            reset_name,
            location: site_location(scenario, &policy.instance_id)?,
            heading_deg: initial.blue_heading_deg_true,
            speed_kts: initial.blue_cruise_speed_kts,
        });
    }
    result.push(Platform {
        name: "Red-1",
        entity_id: 10,
        force_id: 2,
        aircraft: "f16",
        control_port: 21120,
        reset_name: "red_1",
        location: site_location(scenario, &initial.red.instance_id)?,
        heading_deg: initial.red_heading_deg_true,
        speed_kts: initial.red_speed_kts,
    });
    Ok(result)
}

fn site_location(scenario: &Scenario, instance_id: &str) -> Result<Lla> {
    let location = &scenario
        .sites
        .iter()
        .find(|site| site.instance_id == instance_id)
        .with_context(|| format!("find site for {instance_id}"))?
        .maneuvers
        .location;
    let values: Vec<f64> = location
        .strip_prefix("LLA:")
        .context("location must start with LLA:")?
        .split(',')
        .map(|part| part.trim().parse())
        .collect::<std::result::Result<_, _>>()
        .context("LLA values must be numeric")?;
    let [latitude_deg, longitude_deg, altitude_m] = values.as_slice() else {
        bail!("LLA must contain exactly three values");
    };
    Ok(Lla {
        latitude_deg: *latitude_deg,
        longitude_deg: *longitude_deg,
        altitude_m: *altitude_m,
    })
}

fn entities_toml(platforms: &[Platform]) -> String {
    let mut result = String::new();
    for (index, platform) in platforms.iter().enumerate() {
        let path = if index == 0 {
            "entities.ownship"
        } else {
            "entities.moving"
        };
        let table = if index == 0 {
            "[entities.ownship]"
        } else {
            "[[entities.moving]]"
        };
        result.push_str(&format!("{table}\nname = {:?}\nentity_id = {}\nsite_id = 1\napplication_id = 1\nforce_id = {}\naircraft = {:?}\n[{path}.entity_type]\nkind = 1\ndomain = 2\ncountry = 225\ncategory = 84\nsubcategory = 1\n[{path}.jsbsim]\ntype = \"Kinematic\"\nlatitude_deg = {:.9}\nlongitude_deg = {:.9}\naltitude_m = {:.3}\nheading_deg = {:.9}\nspeed_kts = {:.3}\n", platform.name, platform.entity_id, platform.force_id, platform.aircraft, platform.location.latitude_deg, platform.location.longitude_deg, platform.location.altitude_m, platform.heading_deg, platform.speed_kts));
        for waypoint in [
            platform.location,
            forward_waypoint(platform.location, platform.heading_deg),
        ] {
            result.push_str(&format!("[[{path}.flight_plan]]\nlatitude_deg = {:.9}\nlongitude_deg = {:.9}\naltitude_m = {:.3}\n", waypoint.latitude_deg, waypoint.longitude_deg, waypoint.altitude_m));
        }
    }
    result
}

fn forward_waypoint(location: Lla, heading_deg: f64) -> Lla {
    // 60 nmi avoids a second hidden geometry source while giving the platform a heading leg.
    // One radian is 3,440.065 nautical miles on the conventional spherical Earth.
    let distance: f64 = 60.0 / 3_440.065;
    let bearing = heading_deg.to_radians();
    let latitude = location.latitude_deg.to_radians();
    let longitude = location.longitude_deg.to_radians();
    let destination_latitude =
        (latitude.sin() * distance.cos() + latitude.cos() * distance.sin() * bearing.cos()).asin();
    let destination_longitude = longitude
        + (bearing.sin() * distance.sin() * latitude.cos())
            .atan2(distance.cos() - latitude.sin() * destination_latitude.sin());
    Lla {
        latitude_deg: destination_latitude.to_degrees(),
        longitude_deg: destination_longitude.to_degrees(),
        altitude_m: location.altitude_m,
    }
}

fn reset_xml(platform: &Platform) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<!-- Generated WGS-84 policy-contract initial condition. -->\n<initialize name=\"reset00\">\n  <ubody unit=\"KTS\"> 0.0 </ubody>\n  <vbody unit=\"KTS\"> 0.0 </vbody>\n  <wbody unit=\"KTS\"> 0.0 </wbody>\n  <latitude unit=\"DEG\"> {:.9} </latitude>\n  <longitude unit=\"DEG\"> {:.9} </longitude>\n  <phi unit=\"DEG\"> 0.0 </phi>\n  <theta unit=\"DEG\"> 0.0 </theta>\n  <psi unit=\"DEG\"> {:.9} </psi>\n  <altitude unit=\"M\"> {:.3} </altitude>\n  <vc unit=\"KTS\"> {:.3} </vc>\n</initialize>\n",
        platform.location.latitude_deg,
        platform.location.longitude_deg,
        platform.heading_deg,
        platform.location.altitude_m,
        platform.speed_kts
    )
}

fn runscript_xml(platform: &Platform) -> String {
    // The stock F-16 has no telnet endpoint. Add the controller's TCP input in
    // a generated run script so the third-party aircraft definition stays intact.
    format!(
        "<?xml version=\"1.0\"?>\n<!-- Generated SuperCell control wrapper for {}. -->\n<runscript name=\"SuperCell {}\">\n  <use aircraft=\"f16\" initialize=\"reset00\"/>\n  <input port=\"{}\"/>\n  <run start=\"0.0\" end=\"10000000.0\" dt=\"0.0025\"/>\n</runscript>\n",
        platform.name, platform.name, platform.control_port
    )
}

#[cfg(test)]
mod tests {
    use super::{Lla, Platform, entities_toml, forward_waypoint, runscript_xml};
    use crate::config::SupercellConfig;

    #[test]
    fn forward_leg_preserves_altitude() {
        let point = forward_waypoint(
            Lla {
                latitude_deg: 35.0,
                longitude_deg: -120.0,
                altitude_m: 9000.0,
            },
            45.0,
        );
        assert!(point.latitude_deg > 35.0 && point.longitude_deg > -120.0);
        assert!(point.latitude_deg < 36.0 && point.longitude_deg < -119.0);
        assert_eq!(point.altitude_m, 9000.0);
    }

    #[test]
    fn generated_entity_block_deserializes_as_supercell_config() {
        let platform = Platform {
            name: "Blue-1",
            entity_id: 1,
            force_id: 1,
            aircraft: "f16",
            control_port: 21110,
            reset_name: "ownship",
            location: Lla {
                latitude_deg: 35.0,
                longitude_deg: -120.0,
                altitude_m: 9000.0,
            },
            heading_deg: 45.0,
            speed_kts: 350.0,
        };
        let script = runscript_xml(&platform);
        let config = format!(
            "{}\n[dis]\nmulticast_addr = \"127.0.0.1\"\nport = 3000\nexercise_id = 1\n",
            entities_toml(&[platform])
        );
        let parsed: SupercellConfig =
            toml::from_str(&config).expect("generated entity config must parse");
        assert_eq!(parsed.entities.ownship.base.entity_id, 1);
        assert!(script.contains("<use aircraft=\"f16\" initialize=\"reset00\"/>"));
        assert!(script.contains("<input port=\"21110\"/>"));
    }
}
