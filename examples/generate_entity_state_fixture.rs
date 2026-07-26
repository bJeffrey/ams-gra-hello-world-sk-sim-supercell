//! Emit a deterministic Entity State PDU used by downstream compatibility tests.

use bytes::BytesMut;
use supercell::dis::build_entity_state_pdu;
use supercell::entity::{DisEntityType, EntityState};

fn main() {
    let state = EntityState {
        latitude_deg: 36.12,
        longitude_deg: -86.67,
        altitude_m: 1000.0,
        altitude_msl_m: 1000.0,
        terrain_elevation_m: 120.0,
        velocity_north_mps: 10.0,
        velocity_east_mps: 5.0,
        velocity_down_mps: -2.0,
        roll_deg: 1.5,
        pitch_deg: 2.5,
        yaw_deg: 45.0,
        entity_id: 7,
        site_id: 1,
        application_id: 2,
        force_id: 1,
        entity_type: DisEntityType {
            kind: 1,
            domain: 2,
            country: 225,
            category: 84,
            subcategory: 1,
            specific: 0,
            extra: 0,
        },
        marking: "Test-7".to_string(),
        ..EntityState::default()
    };

    let pdu = build_entity_state_pdu(&state, 42, 0x1234_5679);
    let mut bytes = BytesMut::with_capacity(pdu.pdu_length().into());
    pdu.serialize(&mut bytes).expect("serialize fixture PDU");

    for byte in bytes {
        print!("{byte:02x}");
    }
    println!();
}
