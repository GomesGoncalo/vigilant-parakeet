#!/usr/bin/env python3
"""
generate_sim_topology.py

Generate many OBU node YAML configs and a simulator config file for quick experiments.

Usage:
  python3 scripts/generate_sim_topology.py --num 100 --topology full-mesh --prefix obu --start-ip 10.0.1. --out-dir scripts/sim_generated

Notes:
- For full-mesh with large N this creates O(N^2) topology entries (may be huge).
- auto_run not implemented; script only writes files.
"""
import argparse
import os
import sys


def parse_args():
    p = argparse.ArgumentParser(description='Generate simulator topology and node YAMLs')
    p.add_argument('--num', type=int, required=True, help='Number of OBUs to generate')
    p.add_argument('--topology', choices=['chain','full-mesh','grid','ring'], default='chain')
    p.add_argument('--prefix', default='obu', help='OBU name prefix')
    p.add_argument('--start-ip', default='10.0.1.', help='IP base (append 1..N)')
    p.add_argument('--out-dir', default='scripts/sim_generated', help='Output directory')
    return p.parse_args()


def write_node_yaml(path, node_type, ip, hello_history=10):
    content = f"""# Generated node config
node_type: {node_type}
hello_history: {hello_history}
hello_periodicity: 5000
ip: {ip}
"""
    with open(path, 'w') as f:
        f.write(content)


def main():
    args = parse_args()
    n = args.num
    topology = args.topology
    prefix = args.prefix
    start_ip = args.start_ip
    out = args.out_dir

    if n < 1:
        print('num must be >= 1', file=sys.stderr)
        sys.exit(2)

    os.makedirs(out, exist_ok=True)
    nodes_dir = os.path.join(out, 'nodes')
    os.makedirs(nodes_dir, exist_ok=True)

    node_names = [f"{prefix}{i+1}" for i in range(n)]

    # Parse start_ip which may be '10.0.', '10.0.1.' or '10.0.1.5'
    s = start_ip.strip()
    if s.endswith('.'):
        s = s[:-1]
    parts = s.split('.')
    if len(parts) < 2 or len(parts) > 4:
        print('start-ip must be like "10.0.", "10.0.1." or "10.0.1.5"', file=sys.stderr)
        sys.exit(2)
    try:
        a = int(parts[0]); b = int(parts[1])
        if len(parts) == 2:
            start_third = 0
            start_fourth = 1
        elif len(parts) == 3:
            start_third = int(parts[2])
            start_fourth = 1
        else:
            start_third = int(parts[2]); start_fourth = int(parts[3])
    except Exception:
        print('invalid start-ip octets', file=sys.stderr)
        sys.exit(2)

    # Create node YAMLs with proper rollover across third/octets.
    # Use 254 usable hosts per /24 (skip .0 and .255) to avoid network/broadcast addresses.
    USABLE_PER_SUBNET = 254
    for i, name in enumerate(node_names):
        offset_hosts = start_third * USABLE_PER_SUBNET + (start_fourth - 1) + i
        third = offset_hosts // USABLE_PER_SUBNET
        fourth = (offset_hosts % USABLE_PER_SUBNET) + 1
        if third > 255 or fourth < 1 or fourth > 254:
            print(f'IP range exceeded at node {name}: computed {a}.{b}.{third}.{fourth}', file=sys.stderr)
            sys.exit(2)
        ip = f"{a}.{b}.{third}.{fourth}"
        path = os.path.join(nodes_dir, f"{name}.yaml")
        write_node_yaml(path, 'Obu', ip)

    # Build simulator config
    sim_cfg_path = os.path.join(out, 'simulator.yaml')
    with open(sim_cfg_path, 'w') as f:
        f.write('# Generated simulator config\n')
        f.write('nodes:\n')
        for name in node_names:
            node_cfg_path = os.path.join('nodes', f"{name}.yaml")
            f.write(f"  {name}:\n    config_path: {node_cfg_path}\n")

        f.write('\n')
        f.write('topology:\n')

        if topology == 'chain':
            for i, a in enumerate(node_names):
                f.write(f"  {a}:\n")
                if i > 0:
                    b = node_names[i-1]
                    f.write(f"    {b}:\n      latency: 0\n      loss: 0\n")
                if i+1 < n:
                    b = node_names[i+1]
                    f.write(f"    {b}:\n      latency: 0\n      loss: 0\n")
        elif topology == 'ring':
            for i, a in enumerate(node_names):
                f.write(f"  {a}:\n")
                b = node_names[(i+1) % n]
                f.write(f"    {b}:\n      latency: 0\n      loss: 0\n")
                b2 = node_names[(i-1) % n]
                f.write(f"    {b2}:\n      latency: 0\n      loss: 0\n")
        elif topology == 'grid':
            # place nodes in rows of size roughly sqrt(n)
            import math
            cols = int(math.ceil(math.sqrt(n)))
            for idx, a in enumerate(node_names):
                r = idx // cols
                c = idx % cols
                f.write(f"  {a}:\n")
                # neighbor offsets
                for dr, dc in ((0,1),(1,0),(-1,0),(0,-1)):
                    nr = r + dr
                    nc = c + dc
                    if 0 <= nr*cols + nc < n:
                        b = node_names[nr*cols + nc]
                        f.write(f"    {b}:\n      latency: 0\n      loss: 0\n")
        elif topology == 'full-mesh':
            # WARNING: O(N^2) entries
            for a in node_names:
                f.write(f"  {a}:\n")
                for b in node_names:
                    if a == b:
                        continue
                    f.write(f"    {b}:\n      latency: 0\n      loss: 0\n")

    print('\nWrote generated files to:', out)
    print('Simulator config:', sim_cfg_path)
    print('Node configs: in', nodes_dir)
    if topology == 'full-mesh':
        est = n * (n - 1)
        print(f"Note: full-mesh will create {est} directed edges (O(N^2)). This may be very large for N={n}.")

if __name__ == '__main__':
    main()
