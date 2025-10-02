# Routing Table Memory Growth Fix

## Issue Summary
**Location:** `rsu_lib/src/control/routing.rs` line 27-57

**Problem:** IndexMap for heartbeat history could grow unbounded with inefficient operations:
- Wraparound logic cleared entire map (line 52)
- `swap_remove_index(0)` is O(n) operation (line 57)
- No explicit bounds checking beyond capacity

## Solution Implemented

### Changes Made

1. **Replaced IndexMap with VecDeque**
   - Changed from: `IndexMap<u32, (Duration, HashMap<MacAddress, Vec<Target>>)>`
   - Changed to: `VecDeque<(u32, Duration, HashMap<MacAddress, Vec<Target>>)>`
   - Added explicit `max_history: usize` field to track configured limit

2. **Improved Data Structure**
   ```rust
   pub struct Routing {
       hb_seq: u32,
       boot: Instant,
       sent: VecDeque<(u32, Duration, HashMap<MacAddress, Vec<Target>>)>,
       max_history: usize,  // NEW: explicit history limit
   }
   ```

3. **Optimized Operations**
   - **Before:** `swap_remove_index(0)` - O(n) complexity
   - **After:** `pop_front()` - O(1) complexity
   - **Before:** IndexMap lookup by key with `get_mut(&key)`
   - **After:** Linear search with `iter_mut().find()` - acceptable given small history size

4. **Fixed Memory Growth**
   ```rust
   // Before: O(n) removal
   if self.sent.len() == self.sent.capacity() && self.sent.capacity() > 0 {
       self.sent.swap_remove_index(0);
   }
   
   // After: O(1) removal with explicit bound checking
   if self.sent.len() >= self.max_history {
       self.sent.pop_front();
   }
   ```

5. **Improved Wraparound Handling**
   ```rust
   // Before: IndexMap first() check
   if self.sent.first().is_some_and(|(x, _)| x > &message.id()) {
       self.sent.clear();
   }
   
   // After: VecDeque front() check with 3-tuple
   if self.sent.front().is_some_and(|(x, _, _)| x > &message.id()) {
       self.sent.clear();
   }
   ```

### Performance Benefits

1. **O(1) Pop Front:** VecDeque provides constant-time removal from the front
2. **Fixed Memory:** Explicit `max_history` ensures bounded growth
3. **Better Cache Locality:** VecDeque has better cache performance for sequential access
4. **Simpler Code:** No need for index-based removal, just `pop_front()`

### Trade-offs

- **Lookup Time:** Changed from O(1) IndexMap lookup to O(n) linear search
- **Acceptable Because:** 
  - History size is typically small (configured via `hello_history`)
  - Lookups only happen on heartbeat replies (not hot path)
  - Sequential iteration for routing decisions already O(n)

## Validation

### Tests Passed
- ✅ All 26 rsu_lib unit tests pass
- ✅ All 230+ workspace tests pass
- ✅ Clippy passes with no warnings
- ✅ Existing behavior preserved (tests confirm)

### Test Coverage
- `can_generate_heartbeat` - validates heartbeat creation
- `rsu_handle_heartbeat_reply_inserts_route` - validates route insertion
- `iter_next_hops_empty_and_get_route_none_when_empty` - validates empty state
- Integration tests confirm multi-hop routing still works

## Code Impact

### Files Modified
- `rsu_lib/src/control/routing.rs` (27 lines, 5 methods updated)

### Methods Updated
1. `Routing::new()` - Initialize VecDeque with max_history
2. `send_heartbeat()` - Use pop_front() instead of swap_remove_index(0)
3. `handle_heartbeat_reply()` - Use iter_mut().find() instead of get_mut(&key)
4. `get_route_to()` - Destructure 3-tuple instead of 2-tuple
5. `iter_next_hops()` - Destructure 3-tuple instead of 2-tuple

## Recommendations Applied

✅ Use circular buffer/ring buffer for fixed-size history
✅ Replaced O(n) `swap_remove_index(0)` operation
✅ Used VecDeque for O(1) `pop_front`

## Additional Benefits

1. **Memory Safety:** Explicit bounds prevent unbounded growth
2. **Predictable Performance:** O(1) operations for common path
3. **Maintainability:** Simpler code with standard library types
4. **Type Safety:** Explicit max_history field documents intent

## Compatibility

- ✅ No breaking API changes
- ✅ Binary protocol unchanged
- ✅ Routing behavior unchanged
- ✅ Configuration parameters unchanged
- ✅ All existing tests pass

## Future Considerations

1. **OBU Routing:** Similar pattern exists in `obu_lib/src/control/routing.rs`
   - Could apply same optimization if needed
   - OBU uses IndexMap in similar way for heartbeat tracking

2. **Monitoring:** Consider adding metrics for:
   - History queue size
   - Pop front operations count
   - Wraparound clear events

3. **Tuning:** `hello_history` parameter now directly controls memory:
   - Each entry: ~(8 + 16 + HashMap size) bytes
   - Typical: 10 entries * ~100 bytes = ~1KB per RSU
