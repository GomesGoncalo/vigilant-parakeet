# Latency Jitter Configuration

## Overview

The simulator now supports configurable latency jitter to simulate realistic network conditions. Jitter adds random variation to the base latency, making the network behave more like real-world wireless links.

## Configuration

Add the `jitter` parameter to any channel in your `simulator.yaml` topology configuration:

```yaml
topology:
  node1:
    node2:
      latency: 10    # Base latency in milliseconds
      loss: 0.01     # Packet loss rate (0.0-1.0)
      jitter: 2      # Jitter range in milliseconds (±2ms)
```

### Jitter Behavior

- **Range**: Jitter is applied as ±N milliseconds around the base latency
- **Example**: `latency: 10ms, jitter: 2ms` produces latencies in range **[8ms, 12ms]**
- **Distribution**: Uniform random distribution within the range
- **Per-packet**: Each packet gets a new random jitter value
- **Non-negative**: Total latency is clamped to never go below 0ms

### Configuration Examples

#### No Jitter (Deterministic)
```yaml
latency: 5
loss: 0
jitter: 0  # or omit the jitter field
```
Result: Every packet has exactly 5ms latency

#### Low Jitter (Stable Link)
```yaml
latency: 10
loss: 0
jitter: 1  # ±1ms
```
Result: Latencies uniformly distributed in [9ms, 11ms]

#### Medium Jitter (Typical Wireless)
```yaml
latency: 20
loss: 0.02
jitter: 5  # ±5ms
```
Result: Latencies uniformly distributed in [15ms, 25ms]

#### High Jitter (Congested/Mobile)
```yaml
latency: 50
loss: 0.1
jitter: 20  # ±20ms
```
Result: Latencies uniformly distributed in [30ms, 70ms]

## Impact on Measurements

### Latency Histograms

With jitter enabled, the latency histogram tool will show:
- **Wider distributions** - Multiple bins populated instead of single peaks
- **More realistic patterns** - Similar to real network measurements
- **Better testing** - Routing algorithms must handle variation

Example histogram output with `jitter: 2` on a 12ms link:

```
Latency Range (μs) | Count | Distribution
-------------------|-------|-------------
   10000 -    10100 |     3 | ###########
   10100 -    10200 |     5 | ###################
   10200 -    10300 |     8 | ##############################
   10300 -    10400 |    12 | ##############################################
   10400 -    10500 |    15 | ##################################################
   10500 -    10600 |    14 | ###############################################
   10600 -    10700 |    11 | ########################################
   10700 -    10800 |     7 | ##########################
   10800 -    10900 |     4 | ###############
   10900 -    11000 |     1 | ####
```

### Routing Behavior

Jitter affects routing decisions:
- **Latency-based routing** will see varying measurements
- **Route stability** is tested under realistic conditions
- **Hysteresis mechanisms** prevent route flapping from jitter
- **Failover logic** is exercised more realistically

## Testing

### Verify Jitter is Working

Run the latency histogram tool to see jitter in action:

```bash
# Run simulator with jitter-enabled config
sudo RUST_LOG=info ./target/release/simulator --config-file examples/simulator.yaml --pretty &

# Measure latencies (use smaller sample count for quick check)
PING_COUNT=20 ./scripts/measure-latency-histogram.sh examples/simulator.yaml
```

Expected results:
- Nodes with `jitter: 0` show tight distributions (1-2 bins)
- Nodes with `jitter: 2` show wider distributions (4-8 bins)
- Larger jitter values produce proportionally wider distributions

### Example Test Scenario

```yaml
topology:
  rsu1:
    obu1:
      latency: 0
      loss: 0
      jitter: 0      # Direct link, no jitter
    obu2:
      latency: 10
      loss: 0
      jitter: 2      # One-hop link, moderate jitter
    obu3:
      latency: 20
      loss: 0.01
      jitter: 5      # Two-hop link, high jitter + loss
```

This configuration creates three distinct link types for testing routing behavior under varying conditions.

## Implementation Details

### Algorithm

For each packet transmission:

1. Read base `latency` and `jitter` from channel parameters
2. If `jitter > 0`:
   - Generate random value in range `[-jitter, +jitter]`
   - Add to base latency
   - Clamp to non-negative (never go below 0ms)
3. Schedule packet delivery after calculated latency

### Performance

- **Minimal overhead**: Random number generation is fast (~nanoseconds)
- **Lock-free fast path**: Jitter calculation done with short-lived read lock
- **Concurrent**: Each channel processes packets independently
- **No blocking**: Jitter doesn't affect other channels

### Thread Safety

- Channel parameters (including jitter) can be updated at runtime via HTTP API
- Parameter changes detected via notification channel
- Packets in flight use jitter value from when they were enqueued

## Default Values

If `jitter` is not specified in the configuration:
- **Default**: 0ms (no jitter)
- **Backward compatible**: Existing configs work without modification

## Troubleshooting

### Jitter Not Visible in Histograms

- **Check config**: Ensure `jitter: N` (N > 0) is set for the channel
- **Increase samples**: Use `PING_COUNT=50` or higher for clearer distributions
- **Verify histogram bins**: Fine bins (20-100μs) are needed to see small jitter
- **Check node types**: Ensure measuring OBU → RSU connections (filtered by script)

### Latency Lower Than Expected

- Remember jitter is ±N around base latency
- Some packets will have `latency - jitter` delay
- Check histogram min/max values to confirm range

### Excessive Variation

- Jitter is uniform random, not Gaussian
- For very long-running tests, all values in range should appear equally
- External factors (CPU load, network namespaces) may add additional variation

## Future Enhancements

Potential improvements (not yet implemented):

- **Gaussian jitter**: More realistic than uniform distribution
- **Correlated jitter**: Model burst patterns in wireless
- **Dynamic jitter**: Change jitter based on channel load
- **One-way vs RTT**: Different jitter for each direction
- **Jitter percentiles**: Configure P50, P90, P99 instead of uniform range

## See Also

- [Latency Histogram Tool](scripts/README.md#measure-latency-histogramsh)
- [Channel Configuration](examples/simulator.yaml)
- [Simulator Architecture](ARCHITECTURE.md)
