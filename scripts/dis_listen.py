#!/usr/bin/env python3
"""
dis_listen.py — DIS multicast listener / decoder
=================================================

Joins a UDP multicast group and decodes every incoming DIS PDU, printing a
human-readable summary to stdout.  Zero external dependencies; uses only the
Python standard library and a compact hand-rolled DIS v6/v7 header + Entity
State PDU parser that matches the wire format produced by SuperCell.

Usage
-----
    python3 scripts/dis_listen.py [OPTIONS]

Options (all optional, defaults match single_entity.toml / scenario.toml)
--------------------------------------------------------------------------
    --addr   MULTICAST_ADDR   default: 239.1.2.3
    --port   PORT             default: 21100
    --iface  LOCAL_IFACE_IP   default: 0.0.0.0  (let the OS choose)
    --once                    exit after the first valid PDU (useful for CI smoke-test)
    --raw                     also dump the raw hex bytes of each PDU
    --timeout SECONDS         stop listening after N seconds; exits 0 (unless --expect-entities fails)
    --expect-entities ID,...  comma-separated entity IDs to assert; exits 0 if all seen, 1 if any missing

Examples
--------
    # Listen with defaults while SuperCell is running:
    python3 scripts/dis_listen.py

    # Custom group / port:
    python3 scripts/dis_listen.py --addr 239.1.2.3 --port 21100

    # Smoke-test: receive one PDU then exit 0:
    python3 scripts/dis_listen.py --once

    # Show raw hex alongside decoded output:
    python3 scripts/dis_listen.py --raw

    # Assert all 11 entity IDs appear within 30 seconds:
    python3 scripts/dis_listen.py --expect-entities 1,2,3,4,5,6,7,8,9,10,11 --timeout 30

    # Graceful stop after 10 s (no entity assertion):
    python3 scripts/dis_listen.py --timeout 10
"""

from __future__ import annotations

import argparse
import math
import socket
import struct
import sys
import time
from dataclasses import dataclass


# ─── DIS constants ────────────────────────────────────────────────────────────

PDU_TYPE_NAMES: dict[int, str] = {
    1:  "Entity State",
    2:  "Fire",
    3:  "Detonation",
    11: "Collision",
    20: "Service Request",
    70: "Signal",
    71: "Transmitter",
    72: "Receiver",
}

FORCE_ID_NAMES: dict[int, str] = {
    0: "Other",
    1: "Friendly",
    2: "Opposing",
    3: "Neutral",
}


# ─── DIS PDU Header (12 bytes, big-endian) ────────────────────────────────────
# Per IEEE 1278.1 all DIS PDUs start with:
#   offset  size  field
#   0       1     protocol_version  u8
#   1       1     exercise_id       u8
#   2       1     pdu_type          u8
#   3       1     protocol_family   u8
#   4       4     timestamp         u32
#   8       2     pdu_length        u16
#  10       2     padding           u16

DIS_HEADER_FMT = ">BBBBIHH"
DIS_HEADER_SIZE = struct.calcsize(DIS_HEADER_FMT)  # == 12


@dataclass
class DisHeader:
    protocol_version: int
    exercise_id: int
    pdu_type: int
    protocol_family: int
    timestamp: int
    pdu_length: int
    padding: int

    @classmethod
    def parse(cls, data: bytes) -> "DisHeader":
        if len(data) < DIS_HEADER_SIZE:
            raise ValueError(f"Too short for DIS header: {len(data)} bytes")
        fields = struct.unpack_from(DIS_HEADER_FMT, data, 0)
        return cls(*fields)

    def pdu_type_name(self) -> str:
        return PDU_TYPE_NAMES.get(self.pdu_type, f"Unknown({self.pdu_type})")


# ─── Entity State PDU body (72 bytes after the 12-byte header) ────────────────
# Layout per IEEE 1278.1-2012 §5.3.3:
#
#  Idx  Field                  Type  Bytes
#  ---  -----                  ----  -----
#   0   site_id                H       2    ─┐
#   1   application_id         H       2     ├ Entity ID
#   2   entity_id              H       2    ─┘
#   3   force_id               B       1
#   4   num_articulation_params B      1
#   5   entity_kind            B       1    ─┐
#   6   entity_domain          B       1     │
#   7   entity_country         H       2     ├ Entity Type
#   8   entity_category        B       1     │
#   9   entity_subcategory     B       1     │
#  10   entity_specific        B       1     │
#  11   entity_extra           B       1    ─┘
#  12   alt_kind               B       1    ─┐
#  13   alt_domain             B       1     │
#  14   alt_country            H       2     ├ Alt Entity Type
#  15   alt_category           B       1     │
#  16   alt_subcategory        B       1     │
#  17   alt_specific           B       1     │
#  18   alt_extra              B       1    ─┘
#  19   vel_x                  f       4    ─┐ Linear Velocity (ECEF m/s)
#  20   vel_y                  f       4     │
#  21   vel_z                  f       4    ─┘
#  22   loc_x                  d       8    ─┐ World Location (ECEF m)
#  23   loc_y                  d       8     │
#  24   loc_z                  d       8    ─┘
#  25   psi   (yaw)            f       4    ─┐ Euler Orientation (radians)
#  26   theta (pitch)          f       4     │
#  27   phi   (roll)           f       4    ─┘
#                                     TOTAL 72 bytes

ENTITY_STATE_BODY_FMT  = ">HHHBBBBHBBBBBBHBBBBfffdddfff"
ENTITY_STATE_BODY_SIZE = struct.calcsize(ENTITY_STATE_BODY_FMT)  # == 72


@dataclass
class EntityStatePdu:
    site_id: int
    application_id: int
    entity_id: int
    force_id: int
    entity_kind: int
    entity_domain: int
    entity_country: int
    entity_category: int
    vel_x: float
    vel_y: float
    vel_z: float
    loc_x: float
    loc_y: float
    loc_z: float
    psi: float    # yaw   (radians)
    theta: float  # pitch (radians)
    phi: float    # roll  (radians)

    @classmethod
    def parse(cls, data: bytes, offset: int = DIS_HEADER_SIZE) -> "EntityStatePdu":
        if len(data) < offset + ENTITY_STATE_BODY_SIZE:
            raise ValueError(
                f"Packet too short for Entity State body: "
                f"need {offset + ENTITY_STATE_BODY_SIZE} bytes, got {len(data)}"
            )
        f = struct.unpack_from(ENTITY_STATE_BODY_FMT, data, offset)
        # Index map — see layout table above
        return cls(
            site_id=f[0], application_id=f[1], entity_id=f[2],
            force_id=f[3],
            entity_kind=f[5], entity_domain=f[6], entity_country=f[7], entity_category=f[8],
            vel_x=f[19], vel_y=f[20], vel_z=f[21],
            loc_x=f[22], loc_y=f[23], loc_z=f[24],
            psi=f[25], theta=f[26], phi=f[27],
        )

    # ── Derived quantities ─────────────────────────────────────────────────────

    def speed_mps(self) -> float:
        return math.sqrt(self.vel_x**2 + self.vel_y**2 + self.vel_z**2)

    def lat_lon_alt(self) -> tuple[float, float, float]:
        """Convert ECEF (x, y, z) back to geodetic (lat°, lon°, alt_m) via iterative Bowring."""
        WGS84_A  = 6_378_137.0
        WGS84_F  = 1.0 / 298.257_223_563
        WGS84_E2 = 2 * WGS84_F - WGS84_F ** 2

        x, y, z = self.loc_x, self.loc_y, self.loc_z
        lon = math.atan2(y, x)
        p   = math.hypot(x, y)

        # Iterative Bowring convergence (5 iterations ≈ sub-millimetre)
        lat = math.atan2(z, p * (1 - WGS84_E2))
        for _ in range(5):
            sin_lat = math.sin(lat)
            N   = WGS84_A / math.sqrt(1 - WGS84_E2 * sin_lat ** 2)
            lat = math.atan2(z + WGS84_E2 * N * sin_lat, p)

        sin_lat = math.sin(lat)
        cos_lat = math.cos(lat)
        N = WGS84_A / math.sqrt(1 - WGS84_E2 * sin_lat ** 2)
        if abs(cos_lat) > 1e-10:
            alt = p / cos_lat - N
        else:
            alt = abs(z) / abs(sin_lat) - N * (1 - WGS84_E2)

        return math.degrees(lat), math.degrees(lon), alt

    def force_name(self) -> str:
        return FORCE_ID_NAMES.get(self.force_id, f"Unknown({self.force_id})")


# ─── Socket helpers ───────────────────────────────────────────────────────────

def _is_multicast(addr: str) -> bool:
    """Return True if addr is in the IPv4 multicast range 224.0.0.0/4."""
    try:
        first_octet = int(addr.split(".")[0])
        return 224 <= first_octet <= 239
    except (ValueError, IndexError):
        return False


def join_multicast(multicast_addr: str, port: int, iface: str) -> socket.socket:
    """Create and return a UDP socket ready to receive DIS PDUs.

    For real multicast addresses (224-239.x.x.x) the socket joins the group
    via IP_ADD_MEMBERSHIP.  For unicast loopback addresses such as 127.0.0.1
    (used by full_scenario.toml on hosts where multicast loopback is broken)
    the socket simply binds to INADDR_ANY:port — the kernel delivers unicast
    datagrams sent to that port without any group join.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    except AttributeError:
        pass  # not available on all platforms

    if _is_multicast(multicast_addr):
        # Bind to INADDR_ANY so the kernel delivers multicast datagrams
        sock.bind(("", port))
        # IP_ADD_MEMBERSHIP: join the multicast group
        mreq = struct.pack("4s4s",
                           socket.inet_aton(multicast_addr),
                           socket.inet_aton(iface))
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
    else:
        # Unicast target — bind to the specific address so we match the
        # destination address of incoming datagrams.  A wildcard bind
        # (0.0.0.0) loses to any socket already bound to the specific
        # address (e.g. another receiver on 127.0.0.1:21100).
        sock.bind((multicast_addr, port))

    return sock


# ─── Pretty-print helpers ─────────────────────────────────────────────────────

BOX_WIDTH = 64

def fmt_entity_state(
    hdr: DisHeader,
    pdu: EntityStatePdu,
    src: tuple[str, int],
    show_raw: bool,
    data: bytes,
) -> str:
    lat, lon, alt = pdu.lat_lon_alt()
    lines = [
        f"┌─ {hdr.pdu_type_name()} PDU "
        f"(protocol_v{hdr.protocol_version}  exercise={hdr.exercise_id}  "
        f"src={src[0]}:{src[1]})",

        f"│  Entity   : site={pdu.site_id}  app={pdu.application_id}  "
        f"id={pdu.entity_id}",

        f"│  Force    : {pdu.force_name()} ({pdu.force_id})",

        f"│  Type     : kind={pdu.entity_kind}  domain={pdu.entity_domain}  "
        f"country={pdu.entity_country}  cat={pdu.entity_category}",

        f"│  Position : lat={lat:+.6f}°  lon={lon:+.6f}°  alt={alt:.1f} m",

        f"│  Speed    : {pdu.speed_mps():.2f} m/s  "
        f"(vx={pdu.vel_x:.2f}  vy={pdu.vel_y:.2f}  vz={pdu.vel_z:.2f})",

        f"│  Orient   : yaw={math.degrees(pdu.psi):.2f}°  "
        f"pitch={math.degrees(pdu.theta):.2f}°  "
        f"roll={math.degrees(pdu.phi):.2f}°",

        f"│  ECEF     : x={pdu.loc_x:.1f}  y={pdu.loc_y:.1f}  z={pdu.loc_z:.1f}",

        f"│  PDU size : {hdr.pdu_length} bytes",
    ]
    if show_raw:
        lines.append(f"│  Raw hex  : {data.hex()}")
    lines.append("└" + "─" * BOX_WIDTH)
    return "\n".join(lines)


def fmt_generic(
    hdr: DisHeader,
    src: tuple[str, int],
    show_raw: bool,
    data: bytes,
) -> str:
    lines = [
        f"┌─ {hdr.pdu_type_name()} PDU "
        f"(protocol_v{hdr.protocol_version}  exercise={hdr.exercise_id}  "
        f"src={src[0]}:{src[1]})",
        f"│  PDU size : {hdr.pdu_length} bytes",
    ]
    if show_raw:
        lines.append(f"│  Raw hex  : {data.hex()}")
    lines.append("└" + "─" * BOX_WIDTH)
    return "\n".join(lines)


# ─── Main loop ────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="DIS multicast listener — join a multicast group and decode incoming PDUs.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--addr", default="239.1.2.3", metavar="ADDR",
        help="Multicast group address (default: 239.1.2.3)",
    )
    parser.add_argument(
        "--port", default=21100, type=int, metavar="PORT",
        help="UDP port (default: 21100)",
    )
    parser.add_argument(
        "--iface", default="0.0.0.0", metavar="IP",
        help="Local interface IP for multicast join (default: 0.0.0.0)",
    )
    parser.add_argument(
        "--once", action="store_true",
        help="Exit with code 0 after receiving the first valid PDU",
    )
    parser.add_argument(
        "--raw", action="store_true",
        help="Include raw hex dump in output",
    )
    parser.add_argument(
        "--timeout", type=float, default=None, metavar="SECONDS",
        help="Stop listening after N seconds (float). Exits 0 unless --expect-entities fails.",
    )
    parser.add_argument(
        "--expect-entities", default=None, metavar="ID,...",
        help="Comma-separated entity IDs to assert. Exits 0 if all seen before timeout, 1 if any missing.",
    )
    args = parser.parse_args()

    # Parse expected entity IDs
    expected_ids: set[int] | None = None
    if args.expect_entities is not None:
        try:
            expected_ids = {int(x.strip()) for x in args.expect_entities.split(",") if x.strip()}
        except ValueError as e:
            sys.exit(f"ERROR: --expect-entities must be comma-separated integers: {e}")
        if not expected_ids:
            sys.exit("ERROR: --expect-entities requires at least one entity ID")
        if args.timeout is None:
            print(
                "WARNING: --expect-entities set without --timeout — listener may run forever.",
                file=sys.stderr,
            )

    print(f"Listening  : udp://{args.addr}:{args.port}  (iface={args.iface})")
    if args.timeout is not None:
        print(f"Timeout    : {args.timeout}s")
    if expected_ids is not None:
        print(f"Expecting  : entity IDs {sorted(expected_ids)}")
    print("Press Ctrl+C to stop.\n")

    try:
        sock = join_multicast(args.addr, args.port, args.iface)
    except OSError as e:
        sys.exit(f"ERROR: could not join multicast group: {e}")

    pdu_count = 0
    start = time.monotonic()
    deadline = (start + args.timeout) if args.timeout is not None else None
    seen_entity_ids: set[int] = set()

    def _report_and_exit() -> None:
        """Print entity-assertion summary and exit with the appropriate code."""
        elapsed = time.monotonic() - start
        print(f"\nStopped after {elapsed:.1f}s — received {pdu_count} PDU(s).")
        if expected_ids is None:
            sys.exit(0)
        missing = expected_ids - seen_entity_ids
        if missing:
            print(f"MISSING ENTITIES: {sorted(missing)}")
            print(f"SEEN ENTITIES   : {sorted(seen_entity_ids)}")
            sys.exit(1)
        else:
            print(f"ALL ENTITIES CONFIRMED: {sorted(seen_entity_ids)}")
            sys.exit(0)

    try:
        while True:
            # Honour timeout: set socket recv timeout to remaining time (max 1 s)
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                sock.settimeout(min(1.0, remaining))
            else:
                sock.settimeout(None)

            try:
                data, src = sock.recvfrom(65535)
            except socket.timeout:
                # Check deadline again at top of loop
                continue

            elapsed = time.monotonic() - start

            try:
                hdr = DisHeader.parse(data)
            except ValueError as e:
                print(f"[{elapsed:.3f}s] BAD PACKET from {src[0]}:{src[1]}: {e}", flush=True)
                continue

            pdu_count += 1
            prefix = f"[{elapsed:.3f}s  #{pdu_count}]"

            if hdr.pdu_type == 1:  # Entity State
                try:
                    pdu = EntityStatePdu.parse(data)
                    seen_entity_ids.add(pdu.entity_id)
                    print(prefix, flush=True)
                    print(fmt_entity_state(hdr, pdu, src, args.raw, data), flush=True)
                except (ValueError, struct.error) as e:
                    print(f"{prefix} Entity State parse error: {e}", flush=True)
                    if args.raw:
                        print(f"  raw: {data.hex()}", flush=True)
            else:
                print(prefix, flush=True)
                print(fmt_generic(hdr, src, args.raw, data), flush=True)

            if args.once:
                elapsed = time.monotonic() - start
                print(f"\nReceived first PDU after {elapsed:.3f}s — exiting (--once).")
                # --once exits 0 regardless of --expect-entities (insufficient data)
                sys.exit(0)

            # Early-exit optimisation: all expected entities already seen
            if expected_ids is not None and expected_ids.issubset(seen_entity_ids):
                elapsed = time.monotonic() - start
                print(f"\nAll expected entities seen after {elapsed:.3f}s.", flush=True)
                break

    except KeyboardInterrupt:
        pass
    finally:
        sock.close()

    _report_and_exit()


if __name__ == "__main__":
    main()
