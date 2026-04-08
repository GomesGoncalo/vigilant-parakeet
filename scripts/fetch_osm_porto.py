#!/usr/bin/env python3
"""
fetch_osm_porto.py — Download the Porto road network from Overpass and convert
it to the osm_cache.json format consumed by simulator/src/mobility/osm.rs.

Usage:
    python3 scripts/fetch_osm_porto.py [--bbox MIN_LAT MAX_LAT MIN_LON MAX_LON] \
                                       [--output osm_cache.json]

Defaults to a central Porto bounding box.  The script tries several Overpass
mirrors in sequence and times out after 60 s per mirror.

Output format (osm_cache.json):
    {
      "nodes": [{"id": 12345, "lat": 41.155, "lon": -8.620}, ...],
      "ways":  [{"id": 99999, "node_ids": [12345, 67890], "oneway": false}, ...]
    }
"""

import argparse
import json
import sys
import time
import urllib.request
import urllib.error
from typing import Any, Dict, List

OVERPASS_MIRRORS = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
]

HIGHWAY_EXCLUDE = "footway|cycleway|path|pedestrian|service|track|steps|corridor"

QUERY_TEMPLATE = """
[out:json][timeout:60];
(
  way["highway"]["highway"!~"{exclude}"]({min_lat},{min_lon},{max_lat},{max_lon});
);
(._;>;);
out body;
""".strip()


def fetch_overpass(query: str, timeout: int = 90) -> Dict[str, Any]:
    data = ("data=" + urllib.parse.quote(query)).encode()
    for mirror in OVERPASS_MIRRORS:
        print(f"  Trying {mirror} ...", end=" ", flush=True)
        try:
            req = urllib.request.Request(
                mirror,
                data=data,
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                body = resp.read()
            print("OK")
            return json.loads(body)
        except Exception as exc:
            print(f"FAILED ({exc})")
    raise RuntimeError("All Overpass mirrors failed — check network connectivity.")


def convert(raw: Dict[str, Any]):
    """Convert raw Overpass JSON to osm_cache.json format."""
    nodes: List[Dict] = []
    ways: List[Dict] = []

    node_lookup: Dict[int, Dict] = {}
    for element in raw.get("elements", []):
        if element["type"] == "node":
            n = {"id": element["id"], "lat": element["lat"], "lon": element["lon"]}
            node_lookup[element["id"]] = n
            nodes.append(n)
        elif element["type"] == "way":
            tags = element.get("tags", {})
            oneway = tags.get("oneway", "no") == "yes"
            ways.append(
                {
                    "id": element["id"],
                    "node_ids": element.get("nodes", []),
                    "oneway": oneway,
                }
            )

    # Remove nodes that are not referenced by any way (pure metadata nodes)
    referenced = {nid for w in ways for nid in w["node_ids"]}
    nodes = [n for n in nodes if n["id"] in referenced]

    return {"nodes": nodes, "ways": ways}


def main():
    import urllib.parse  # needed for quote inside fetch_overpass

    parser = argparse.ArgumentParser(description="Fetch Porto OSM road data.")
    parser.add_argument("--min-lat", type=float, default=41.145)
    parser.add_argument("--max-lat", type=float, default=41.165)
    parser.add_argument("--min-lon", type=float, default=-8.630)
    parser.add_argument("--max-lon", type=float, default=-8.610)
    parser.add_argument("--output", default="osm_cache.json")
    args = parser.parse_args()

    query = QUERY_TEMPLATE.format(
        exclude=HIGHWAY_EXCLUDE,
        min_lat=args.min_lat,
        max_lat=args.max_lat,
        min_lon=args.min_lon,
        max_lon=args.max_lon,
    )

    print(
        f"Fetching Porto roads for bbox "
        f"({args.min_lat}, {args.max_lat}, {args.min_lon}, {args.max_lon}) ..."
    )
    t0 = time.time()
    raw = fetch_overpass(query)
    elapsed = time.time() - t0
    print(f"Downloaded in {elapsed:.1f}s — {len(raw.get('elements', []))} elements")

    cache = convert(raw)
    print(f"Converted: {len(cache['nodes'])} nodes, {len(cache['ways'])} ways")

    with open(args.output, "w") as f:
        json.dump(cache, f)
    print(f"Written to {args.output}")


if __name__ == "__main__":
    main()
