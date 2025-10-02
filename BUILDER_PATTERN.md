# Builder Pattern Implementation

## Overview

This document describes the Builder pattern implementation for `Obu` and `Rsu` initialization in `obu_lib` and `rsu_lib`. The Builder pattern provides a flexible, ergonomic API for constructing nodes with complex configuration.

## Motivation

The previous initialization approach using `new()` methods had several limitations:
- Required passing all dependencies explicitly
- Made testing more difficult with partial configuration
- Less discoverable API for optional parameters
- No clear way to provide sensible defaults

The Builder pattern addresses these issues by:
- Providing fluent, chainable API for configuration
- Supporting partial configuration with sensible defaults
- Separating test and production concerns cleanly
- Making optional parameters explicit and discoverable

## Usage Examples

### OBU Builder

#### Basic Usage (Production)

```rust
use obu_lib::ObuBuilder;

// Minimal configuration with defaults
let obu = ObuBuilder::new("eth0")
    .with_ip("192.168.1.100".parse()?)
    .build()?;
```

#### Full Configuration

```rust
use obu_lib::ObuBuilder;

let obu = ObuBuilder::new("eth0")
    .with_tap_name("tap0")
    .with_ip("192.168.1.100".parse()?)
    .with_mtu(1500)
    .with_hello_history(20)
    .with_cached_candidates(5)
    .with_encryption(true)
    .build()?;
```

#### Testing with Mock Devices

```rust
use obu_lib::ObuBuilder;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

// In test code with test_helpers feature enabled
let (tun1, tun2) = Tun::pair()?;
let (dev1, dev2) = Device::pair()?;

let obu = ObuBuilder::new("test")
    .with_encryption(false)
    .with_tun(Arc::new(tun1))
    .with_device(Arc::new(dev1))
    .build()?;
```

#### From Existing Args

```rust
use obu_lib::{ObuArgs, ObuBuilder};

// Convert from command-line args or config
let args = ObuArgs::parse(); // from clap
let obu = ObuBuilder::from_args(args).build()?;

// Or modify existing args
let obu = ObuBuilder::from_args(args)
    .with_encryption(true)
    .build()?;
```

### RSU Builder

#### Basic Usage (Production)

```rust
use rsu_lib::RsuBuilder;

// Minimal configuration - hello_periodicity is required
let rsu = RsuBuilder::new("eth0", 5000)  // 5 second period
    .with_ip("192.168.1.1".parse()?)
    .build()?;
```

#### Full Configuration

```rust
use rsu_lib::RsuBuilder;

let rsu = RsuBuilder::new("eth0", 5000)
    .with_tap_name("tap0")
    .with_ip("192.168.1.1".parse()?)
    .with_mtu(1500)
    .with_hello_history(20)
    .with_hello_periodicity(3000)  // Override to 3 seconds
    .with_cached_candidates(5)
    .with_encryption(true)
    .build()?;
```

#### Testing with Mock Devices

```rust
use rsu_lib::RsuBuilder;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

// In test code with test_helpers feature enabled
let (tun1, tun2) = Tun::pair()?;
let (dev1, dev2) = Device::pair()?;

let rsu = RsuBuilder::new("test", 5000)
    .with_encryption(false)
    .with_tun(Arc::new(tun1))
    .with_device(Arc::new(dev1))
    .build()?;
```

## API Reference

### ObuBuilder

#### Constructor Methods

- `ObuBuilder::new(bind: impl Into<String>)` - Create builder with required bind interface
- `ObuBuilder::from_args(args: ObuArgs)` - Create builder from existing args

#### Configuration Methods

All methods return `Self` for chaining:

- `with_tap_name(name: impl Into<String>)` - Set TAP device name
- `with_ip(ip: Ipv4Addr)` - Set IP address
- `with_mtu(mtu: i32)` - Set MTU (default: 1436)
- `with_hello_history(history: u32)` - Set hello history size (default: 10)
- `with_cached_candidates(count: u32)` - Set cached upstream candidates (default: 3)
- `with_encryption(enabled: bool)` - Enable/disable encryption (default: false)
- `with_tun(tun: Arc<Tun>)` - Inject test TUN device (test mode only)
- `with_device(device: Arc<Device>)` - Inject test Device (test mode only)

#### Build Method

- `build()` -> `Result<Arc<Obu>>` - Construct the Obu instance

### RsuBuilder

#### Constructor Methods

- `RsuBuilder::new(bind: impl Into<String>, hello_periodicity: u32)` - Create builder with required parameters
- `RsuBuilder::from_args(args: RsuArgs)` - Create builder from existing args

#### Configuration Methods

All methods return `Self` for chaining:

- `with_tap_name(name: impl Into<String>)` - Set TAP device name
- `with_ip(ip: Ipv4Addr)` - Set IP address
- `with_mtu(mtu: i32)` - Set MTU (default: 1436)
- `with_hello_history(history: u32)` - Set hello history size (default: 10)
- `with_hello_periodicity(period_ms: u32)` - Set hello broadcast period in milliseconds
- `with_cached_candidates(count: u32)` - Set cached upstream candidates (default: 3)
- `with_encryption(enabled: bool)` - Enable/disable encryption (default: false)
- `with_tun(tun: Arc<Tun>)` - Inject test TUN device (test mode only)
- `with_device(device: Arc<Device>)` - Inject test Device (test mode only)

#### Build Method

- `build()` -> `Result<Arc<Rsu>>` - Construct the Rsu instance

## Implementation Details

### Production vs Test Mode

The builder implementation uses conditional compilation to provide different behavior in test and production modes:

**Production Mode** (`not(any(test, feature = "test_helpers"))`):
- `build()` creates real TUN and Device instances
- `with_tun()` and `with_device()` are not available
- Automatically handles TUN device creation with proper configuration

**Test Mode** (`any(test, feature = "test_helpers")`):
- `build()` requires TUN and Device to be provided via `with_tun()` and `with_device()`
- Returns error if these are not set, guiding test authors to proper usage
- Allows full control over device behavior for testing

### Default Values

| Parameter | OBU Default | RSU Default |
|-----------|-------------|-------------|
| MTU | 1436 | 1436 |
| Hello History | 10 | 10 |
| Cached Candidates | 3 | 3 |
| Encryption | false | false |
| Hello Periodicity | N/A | Required parameter |

### Backward Compatibility

The existing `create()` and `create_with_vdev()` functions are still available and now internally use the builders. This provides a migration path without breaking existing code.

## Testing Benefits

The Builder pattern significantly improves testability:

1. **Partial Configuration**: Only set what's relevant for each test
2. **Clear Intent**: Method names make test setup self-documenting
3. **Type Safety**: Compiler enforces proper test device injection
4. **Isolation**: Easy to create multiple nodes with slight variations

Example test setup:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_obu_routing() {
        let (tun1, tun2) = Tun::pair().unwrap();
        let (dev1, dev2) = Device::pair().unwrap();
        
        let obu = ObuBuilder::new("test")
            .with_hello_history(5)  // Smaller history for faster test
            .with_encryption(false)  // Disable for easier inspection
            .with_tun(Arc::new(tun1))
            .with_device(Arc::new(dev1))
            .build()
            .unwrap();
        
        // Test logic here...
    }
}
```

## Migration Guide

### For Production Code

If you were using `create()`:
```rust
// Old
let obu = obu_lib::create(args)?;

// New (still works, now uses builder internally)
let obu = obu_lib::create(args)?;

// Or migrate to builder directly
let obu = ObuBuilder::from_args(args).build()?;
```

### For Test Code

If you were using `create_with_vdev()`:
```rust
// Old
let obu = obu_lib::create_with_vdev(args, tun, device)?;

// New (still works, uses builder internally)
let obu = obu_lib::create_with_vdev(args, tun, device)?;

// Or use builder directly (recommended)
let obu = ObuBuilder::from_args(args)
    .with_tun(tun)
    .with_device(device)
    .build()?;

// Or build from scratch with only what you need
let obu = ObuBuilder::new("test")
    .with_encryption(true)
    .with_tun(tun)
    .with_device(device)
    .build()?;
```

## Best Practices

1. **Use defaults when possible**: Only override what's necessary for your use case
2. **Make intent clear**: Chain methods to show configuration purpose
3. **Test with realistic values**: Don't use extreme values unless testing edge cases
4. **Group related settings**: Chain related configuration together
5. **Document deviations**: Comment why you're overriding defaults

Example of good builder usage:
```rust
// Good: Clear intent, reasonable values, grouped config
let obu = ObuBuilder::new("eth0")
    // Network configuration
    .with_ip("10.0.0.2".parse()?)
    .with_mtu(1500)
    // Routing configuration
    .with_hello_history(20)  // Longer history for stable network
    .with_cached_candidates(5)  // More candidates for redundancy
    // Security
    .with_encryption(true)
    .build()?;
```

## Performance Considerations

The Builder pattern has minimal runtime overhead:
- All configuration is done before the node is created
- No additional allocations compared to direct construction
- The builder itself is lightweight (all primitive types)
- Cloning a builder is cheap (for reusing common configurations)

## Future Enhancements

Potential future additions to the builder:
- Validation methods (e.g., `validate()` before `build()`)
- Configuration presets (e.g., `ObuBuilder::production()`, `ObuBuilder::testing()`)
- Async builder methods for asynchronous resource initialization
- Builder state types to enforce required parameters at compile time

## Conclusion

The Builder pattern provides a modern, ergonomic API for node construction that:
- Makes common cases simple
- Makes complex cases possible
- Improves testability significantly
- Maintains backward compatibility
- Follows Rust best practices and idioms

For more examples, see the test suites in `obu_lib/src/builder.rs` and `rsu_lib/src/builder.rs`.
