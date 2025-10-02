# Logging Improvements

This document summarizes the logging improvements made to enhance debugging capabilities while reducing noise.

## Overview

The logging system has been improved to provide more useful debugging information with better structure and reduced verbosity. Key improvements include:

1. **Better structured logging** - Added relevant context fields (sizes, MACs, counts)
2. **Appropriate log levels** - Moved verbose logs to debug/trace, kept important events at info/warn/error
3. **Removed noise** - Eliminated redundant or useless log messages
4. **Enhanced error context** - Added operation context and metrics to error messages

## Changes by Component

### Simulator (simulator/src/main.rs)
- **IMPROVED**: Node configuration log now uses `debug` level instead of `info`
  - Changed: `tracing::info!(?settings, "settings")` 
  - To: `tracing::debug!(?settings, "Node configuration loaded")`
  - Rationale: Configuration details are verbose and only needed during debugging

### OBU Initialization (obu_lib/src/control/mod.rs)
- **IMPROVED**: Setup log now includes structured fields instead of debug output
  - Before: `tracing::info!(?obu.args, "Setup Obu")`
  - After: Includes bind, MAC, MTU, hello_history, cached_candidates, encryption status
  - Added context: Makes it easy to identify node configuration at a glance

### RSU Initialization (rsu_lib/src/control/mod.rs)
- **IMPROVED**: Setup log now includes structured fields
  - Before: `tracing::info!(?rsu.args, "Setup Rsu")`
  - After: Includes bind, MAC, MTU, hello_history, hello_period_ms, cached_candidates, encryption
  - Added context: Clear visibility of RSU configuration parameters

### Encryption Errors
- **IMPROVED**: Encryption failure logging (obu_lib/src/control/mod.rs)
  - Added: payload size, structured error field
  - Changed level: error → error (kept) for encryption, warn for decryption
  - Rationale: Encryption failures indicate system issues; decryption failures may be from malformed packets

### Message Parsing Errors
- **IMPROVED**: Parse failure logging (obu_lib and rsu_lib control/mod.rs)
  - Before: `tracing::trace!(..., "could not parse message at offset")`
  - After: `tracing::trace!(..., "Failed to parse message, stopping batch processing")`
  - Added: total_size field for full context
  - Kept at trace level: Parse failures are common in real networks (noise, malformed packets, other protocols) and don't need visibility at debug level where route updates appear

### Device Send/Receive Errors
- **IMPROVED**: All device I/O errors now include structured fields
  - Added: buffer size, packet count, total bytes
  - Changed format: `error = %e` for cleaner display
  - Locations: node_lib, obu_lib, rsu_lib control/node.rs
  - Example: `tracing::error!(error = %e, size = buf.len(), "Failed to send to device")`

### Batch Send Errors
- **IMPROVED**: Batch operations now log metrics
  - Added: packet_count, total_bytes
  - Locations: All handle_messages_batched implementations
  - Rationale: Helps diagnose throughput issues

### OBU Failover Logging
- **IMPROVED**: Upstream failover messages (obu_lib/src/control/node.rs)
  - Before: Generic "promoted cached upstream" info log
  - After: Structured log with new_upstream MAC address
  - Added: Debug log for failover attempt
  - Fixed: Lock poisoning messages are now proper errors with clear text

### RSU Heartbeat Errors
- **IMPROVED**: Heartbeat send logging (rsu_lib/src/control/mod.rs)
  - Added: Success trace with bytes_sent
  - Enhanced error: Includes message size
  - Changed: "RSU failed to send heartbeat" → "Failed to send heartbeat"

### Routing Logs
- **IMPROVED**: Heartbeat reply handling (rsu_lib/src/control/routing.rs)
  - Before: `tracing::warn!("outdated heartbeat")`
  - After: `tracing::debug!(..., "Ignoring outdated heartbeat reply")` with reply_id and sender
  - Rationale: Outdated replies are normal in wireless networks, not warnings

### Removed Noise
- **REMOVED**: Transaction trace logs in wire_traffic processing
  - Removed: `tracing::trace!(has_response, incoming, outgoing, "transaction")`
  - Rationale: These logs generated massive output with limited debugging value
  
- **REMOVED**: "outgoing from tap" trace logs
  - Removed from both OBU and RSU tap traffic processing
  - Rationale: Redundant with other flow tracking mechanisms

- **REMOVED**: Redundant cache selection traces
  - Removed: `"heartbeat: select_and_cache_upstream"` and similar
  - Rationale: The route change logs already capture important selection events

- **REMOVED**: Verbose cache candidate debug log with duplicate calculations
  - Simplified logic in obu_lib/src/control/routing.rs
  - Rationale: Calculation details not useful without verbose debugging feature

- **REMOVED**: "clearing cached_upstream" trace
  - Rationale: Cache clears are tracked via metrics when stats feature is enabled

### Visualization (visualization/src/main.rs)
- **IMPROVED**: Browser console logging reduced and more meaningful
  - Channel updates: info → debug with link_count
  - Node updates: info → debug with node count (only logged on change)
  - Parse errors: info → warn with descriptive messages
  - Parameter changes: info → debug with structured fields

### Wire Traffic Errors
- **IMPROVED**: RSU wire traffic error (rsu_lib/src/control/mod.rs)
  - Before: `tracing::error!("Error in wire_traffic: {:?}", e)`
  - After: `tracing::error!(error = %e, "Wire traffic processing error")`

## Routing Events

### Route Discovery and Changes (INFO Level)
- **PROMOTED**: Route discovery and changes now at INFO level (was DEBUG)
  - Route discovery: First time a route to a destination is found
  - Route changes: When the next hop for a destination changes
  - Added fields: `hops`, `old_hops`, `new_hops` for better context
  - Rationale: These are significant topology events that should be visible at default INFO level
  - Example: `Route discovered: from=XX:XX:XX:XX:XX:XX, to=YY:YY:YY:YY:YY:YY, through=ZZ:ZZ:ZZ:ZZ:ZZ:ZZ, hops=2`

### Upstream Selection (INFO Level)
- **ADDED**: OBU upstream selection logging (obu_lib/src/control/routing.rs)
  - Logs when OBU first selects an upstream path to an RSU
  - Only logged once when first upstream is cached (not on every selection)
  - Fields: upstream MAC, source MAC, hop count
  - Rationale: Major milestone for OBU - indicates it can now send data
  - Example: `Upstream selected: upstream=AA:AA:AA:AA:AA:AA, source=BB:BB:BB:BB:BB:BB, hops=1`

### Loop Detection (WARN Level)
- **ADDED**: Routing loop detection logging (obu_lib/src/control/routing.rs)
  - Logs when a routing loop is detected and packet is dropped
  - Includes the problematic MACs involved in the loop
  - Fields: pkt_from, message_sender, next_upstream
  - Rationale: Critical routing issue that needs visibility
  - Example: `Routing loop detected, dropping packet: pkt_from=XX, message_sender=YY, next_upstream=YY`

### Loop Prevention (DEBUG Level)
- **ADDED**: Skip-forward decision logging (obu_lib/src/control/routing.rs)
  - Logs when heartbeat reply forwarding is skipped to prevent loops
  - Only logs "skip_forward" actions (prevents noise from normal forwards)
  - Rationale: Shows the routing algorithm is working correctly to prevent loops
  - Example: `Skipping forward to prevent loop: pkt_from=XX, message_sender=YY, next_upstream=XX`

## Logging Best Practices Applied

1. **Structured Logging**: Use tracing's structured field syntax (`field = value`)
2. **Appropriate Levels**:
   - `error`: Failures requiring attention (I/O errors, crypto failures)
   - `warn`: Unexpected but recoverable situations (invalid input, deprecated usage)
   - `info`: Key lifecycle and topology events (initialization, route discovery/changes, upstream failover)
   - `debug`: Detailed operational info (outdated packets, parameter updates, adjacency changes)
   - `trace`: Very verbose debugging (parse failures, heartbeat sends, individual packets)

3. **Context Fields**: Always include relevant context:
   - Sizes for data operations
   - MAC addresses for routing/forwarding
   - Counts for batch operations
   - Error details with `%e` format

4. **Clear Messages**: Descriptive message text that indicates:
   - What operation was attempted
   - What component is involved
   - What the outcome was

## Summary of Key Events by Log Level

### INFO Level (topology and lifecycle events)
- **Node initialization**: OBU/RSU startup with configuration
- **Route discovered**: First path to a destination
- **Route changed**: Next hop changed (topology shift)
- **Upstream selected**: OBU first selects upstream path (ready for data)
- **Upstream failover**: OBU switches to next cached upstream after failure

### WARN Level (recoverable issues)
- **Routing loop detected**: Loop detected, packet dropped
- **Decryption failures**: Payload decryption failed (malformed/wrong key)
- **Parse failures in visualization**: Invalid input from user

### ERROR Level (failures requiring attention)
- **Send/receive failures**: I/O errors on device or TAP
- **Encryption failures**: Payload encryption failed (system issue)
- **Lock poisoning**: Routing table lock poisoned (panic recovery)

### DEBUG Level (detailed operations)
- **Skip-forward decision**: Prevented loop by not forwarding
- **Outdated heartbeat replies**: Normal in wireless networks
- **Parameter updates**: Channel parameters changed
- **Node list changes**: Visualization node count changed

### TRACE Level (very verbose)
- **Parse failures**: Invalid protocol markers from network noise
- **Heartbeat sends**: Individual heartbeat transmissions
- **Wire traffic recv**: Raw packet reception details

## Future Improvements (Not Implemented)

These were considered but deferred for separate work:

1. **Feature-gated verbose logging**: Add a `verbose-logging` feature flag for extremely detailed trace logs
2. **Node identity in logs**: Add node MAC/name to all log statements via tracing spans
3. **Metrics integration**: Surface more operational metrics via the stats feature
4. **Log rate limiting**: Prevent flooding logs during network storms
5. **Periodic stats summary**: Log routing table size, neighbor count every N seconds

## Validation

All changes have been validated:
- ✅ `cargo build --workspace --release` - builds successfully
- ✅ `cargo test --workspace` - all tests pass
- ✅ `cargo clippy --workspace --all-targets -- -D warnings` - no warnings
