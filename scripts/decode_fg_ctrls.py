#!/usr/bin/env python3
"""Listen for FGNetCtrls UDP packets and decode them.

Usage:
    python3 scripts/decode_fg_ctrls.py [port]

Default port: 21201
"""

import socket
import struct
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 21201
FG_MAX_ENGINES = 4
FG_MAX_TANKS = 8

def decode_ctrls(buf):
    """Decode FGNetCtrls from big-endian buffer, following net_ctrls.hxx v27."""
    o = 0

    def u32():
        nonlocal o
        v = struct.unpack_from('>I', buf, o)[0]
        o += 4
        return v

    def f64():
        nonlocal o
        v = struct.unpack_from('>d', buf, o)[0]
        o += 8
        return v

    def u32_arr(n):
        nonlocal o
        vals = [struct.unpack_from('>I', buf, o + i*4)[0] for i in range(n)]
        o += 4 * n
        return vals

    def f64_arr(n):
        nonlocal o
        vals = [struct.unpack_from('>d', buf, o + i*8)[0] for i in range(n)]
        o += 8 * n
        return vals

    def pad(n):
        nonlocal o
        o += n

    d = {}
    d['version'] = u32()
    pad(4)  # align to 8-byte for first f64

    # Aero controls
    d['aileron'] = f64()
    d['elevator'] = f64()
    d['rudder'] = f64()
    d['aileron_trim'] = f64()
    d['elevator_trim'] = f64()
    d['rudder_trim'] = f64()
    d['flaps'] = f64()
    d['spoilers'] = f64()
    d['speedbrake'] = f64()

    # Aero control faults
    d['flaps_power'] = u32()
    d['flap_motor_ok'] = u32()

    # Engine controls
    d['num_engines'] = u32()
    pad(4)  # align
    d['master_bat'] = u32_arr(FG_MAX_ENGINES)
    d['master_alt'] = u32_arr(FG_MAX_ENGINES)
    d['magnetos'] = u32_arr(FG_MAX_ENGINES)
    d['starter_power'] = u32_arr(FG_MAX_ENGINES)
    d['throttle'] = f64_arr(FG_MAX_ENGINES)
    d['mixture'] = f64_arr(FG_MAX_ENGINES)
    d['condition'] = f64_arr(FG_MAX_ENGINES)
    d['fuel_pump_power'] = u32_arr(FG_MAX_ENGINES)
    d['prop_advance'] = f64_arr(FG_MAX_ENGINES)
    d['feed_tank_to'] = u32_arr(4)
    d['reverse'] = u32_arr(4)

    # Engine faults
    d['engine_ok'] = u32_arr(FG_MAX_ENGINES)
    d['mag_left_ok'] = u32_arr(FG_MAX_ENGINES)
    d['mag_right_ok'] = u32_arr(FG_MAX_ENGINES)
    d['spark_plugs_ok'] = u32_arr(FG_MAX_ENGINES)
    d['oil_press_status'] = u32_arr(FG_MAX_ENGINES)
    d['fuel_pump_ok'] = u32_arr(FG_MAX_ENGINES)

    # Fuel management
    d['num_tanks'] = u32()
    d['fuel_selector'] = u32_arr(FG_MAX_TANKS)
    d['xfer_pump'] = u32_arr(5)
    d['cross_feed'] = u32()
    pad(4)  # align before f64

    # Brake controls
    d['brake_left'] = f64()
    d['brake_right'] = f64()
    d['copilot_brake_left'] = f64()
    d['copilot_brake_right'] = f64()
    d['brake_parking'] = f64()

    # Landing gear
    d['gear_handle'] = u32()

    # Switches
    d['master_avionics'] = u32()
    # gear_handle(4) + master_avionics(4) = 8 bytes, already 8-byte aligned. No pad.

    # nav and comm
    d['comm_1'] = f64()
    d['comm_2'] = f64()
    d['nav_1'] = f64()
    d['nav_2'] = f64()

    # wind and turbulence
    d['wind_speed_kt'] = f64()
    d['wind_dir_deg'] = f64()
    d['turbulence_norm'] = f64()

    # temp and pressure
    d['temp_c'] = f64()
    d['press_inhg'] = f64()

    # environment
    d['hground'] = f64()
    d['magvar'] = f64()

    # hazards
    d['icing'] = u32()

    # simulation control
    d['speedup'] = u32()
    d['freeze'] = u32()

    # reserved
    RESERVED_SPACE = 25
    d['reserved'] = u32_arr(RESERVED_SPACE)

    d['_bytes_consumed'] = o
    return d


def main():
    # Stop supercell first so we can bind the port
    print(f"Listening for FGNetCtrls on UDP port {PORT}...")
    print("(Make sure supercell is stopped so this can bind the port)")
    print()

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(('0.0.0.0', PORT))

    count = 0
    while True:
        data, addr = sock.recvfrom(2048)
        count += 1

        try:
            d = decode_ctrls(data)
        except Exception as e:
            print(f"[{count}] {len(data)} bytes from {addr} — DECODE ERROR: {e}")
            continue

        consumed = d['_bytes_consumed']
        remaining = len(data) - consumed

        # Print summary
        if count <= 3 or count % 10 == 0:
            print(f"[{count}] {len(data)} bytes, consumed={consumed}, remaining={remaining}")
            print(f"  version={d['version']}  num_engines={d['num_engines']}  num_tanks={d['num_tanks']}")
            print(f"  aileron={d['aileron']:+.4f}  elevator={d['elevator']:+.4f}  rudder={d['rudder']:+.4f}")
            print(f"  throttle={[f'{v:.3f}' for v in d['throttle']]}")
            print(f"  mixture={[f'{v:.3f}' for v in d['mixture']]}")
            print(f"  flaps={d['flaps']:.3f}  gear={d['gear_handle']}  brake_L={d['brake_left']:.3f}  brake_R={d['brake_right']:.3f}")
            print(f"  comm_1={d['comm_1']:.3f}  nav_1={d['nav_1']:.3f}  temp_c={d['temp_c']:.1f}  press={d['press_inhg']:.2f}")
            print(f"  speedup={d['speedup']}  freeze={d['freeze']}  icing={d['icing']}")
            print()
        else:
            # One-liner for most packets
            print(f"[{count}] ail={d['aileron']:+.3f} ele={d['elevator']:+.3f} rud={d['rudder']:+.3f} thr={d['throttle'][0]:.3f} mix={d['mixture'][0]:.3f}")


if __name__ == '__main__':
    main()
