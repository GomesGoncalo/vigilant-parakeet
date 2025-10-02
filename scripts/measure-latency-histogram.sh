#!/usr/bin/env bash
# Measure latency between all node pairs and create a histogram
# Usage: ./scripts/measure-latency-histogram.sh [config.yaml]

set -e

CONFIG_FILE="${1:-simulator.yaml}"
PING_COUNT="${PING_COUNT:-20}"
OUTPUT_DIR="${OUTPUT_DIR:-/tmp/latency_histogram}"
HISTOGRAM_FILE="${OUTPUT_DIR}/histogram.txt"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

mkdir -p "$OUTPUT_DIR"

echo -e "${BLUE}=== Network Latency Histogram Tool ===${NC}"
echo "Config file: $CONFIG_FILE"
echo "Ping count per pair: $PING_COUNT"
echo "Output directory: $OUTPUT_DIR"
echo ""

# Extract node names from config
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Error: Config file '$CONFIG_FILE' not found"
    exit 1
fi

# Parse YAML to get node list (only immediate children under nodes:, indented by exactly 2 spaces)
NODES=$(awk '
BEGIN { in_nodes_section = 0 }
/^nodes:/ { in_nodes_section = 1; next }
/^[a-zA-Z]/ { in_nodes_section = 0 }
in_nodes_section && /^  [a-z0-9_-]+:/ && !/^    / { 
    gsub(/:/, "")
    gsub(/^  /, "")
    print $1
}
' "$CONFIG_FILE" || true)

if [ -z "$NODES" ]; then
    echo "Error: No nodes found in config file"
    exit 1
fi

NODE_ARRAY=($NODES)
NODE_COUNT=${#NODE_ARRAY[@]}

echo -e "${GREEN}Found $NODE_COUNT nodes:${NC} ${NODE_ARRAY[*]}"
echo ""

# Get IP addresses and node types for each node
declare -A NODE_IPS
declare -A NODE_TYPES
echo -e "${YELLOW}Discovering node IP addresses and types from simulator API...${NC}"

# Query simulator API for node types
SIMULATOR_API="${SIMULATOR_API:-http://localhost:3030}"
node_info_json=$(curl -s "$SIMULATOR_API/node_info" 2>/dev/null)

if [ -z "$node_info_json" ]; then
    echo "Error: Could not query simulator API at $SIMULATOR_API/node_info"
    echo "Make sure the simulator is running with HTTP API enabled."
    exit 1
fi

# Parse node types from API response
for node in "${NODE_ARRAY[@]}"; do
    node_type=$(echo "$node_info_json" | jq -r ".${node}.node_type // \"Unknown\"" 2>/dev/null)
    NODE_TYPES[$node]=$node_type
done

# Get IP addresses from network namespaces
for node in "${NODE_ARRAY[@]}"; do
    ns="sim_ns_$node"
    node_type="${NODE_TYPES[$node]}"
    
    # Try to get IP from the tun/tap interface
    ip_addr=$(sudo ip netns exec "$ns" ip -4 addr show | grep -oP '(?<=inet\s)\d+(\.\d+){3}' | head -n 1 || echo "")
    
    if [ -z "$ip_addr" ]; then
        echo "Warning: Could not find IP for node $node in namespace $ns"
    else
        NODE_IPS[$node]=$ip_addr
        echo "  $node ($node_type) -> $ip_addr"
    fi
done
echo ""

# Collect latency measurements (OBU -> RSU only)
echo -e "${YELLOW}Measuring latency from OBU nodes to RSU nodes...${NC}"
LATENCY_FILE="${OUTPUT_DIR}/raw_latencies.txt"
> "$LATENCY_FILE"  # Clear file

# Count OBU -> RSU pairs
obu_count=0
rsu_count=0
for node in "${NODE_ARRAY[@]}"; do
    if [ "${NODE_TYPES[$node]}" = "Obu" ]; then
        obu_count=$((obu_count + 1))
    elif [ "${NODE_TYPES[$node]}" = "Rsu" ]; then
        rsu_count=$((rsu_count + 1))
    fi
done
total_pairs=$((obu_count * rsu_count))
current_pair=0

echo "OBU nodes: $obu_count, RSU nodes: $rsu_count, Total pairs: $total_pairs"
echo "Running pings concurrently..."
echo ""

# Create temporary directory for concurrent results
TEMP_DIR="${OUTPUT_DIR}/tmp_$$"
mkdir -p "$TEMP_DIR"

# Create per-node latency files first
for src_node in "${NODE_ARRAY[@]}"; do
    if [ "${NODE_TYPES[$src_node]}" = "Obu" ]; then
        node_latency_file="${OUTPUT_DIR}/${src_node}_latencies.txt"
        > "$node_latency_file"
    fi
done

# Launch ALL pings simultaneously in one batch for maximum concurrency
ping_pids=()
for src_node in "${NODE_ARRAY[@]}"; do
    src_type="${NODE_TYPES[$src_node]}"
    
    # Skip if source is not an OBU
    if [ "$src_type" != "Obu" ]; then
        continue
    fi
    
    src_ns="sim_ns_$src_node"
    src_ip="${NODE_IPS[$src_node]}"
    
    if [ -z "$src_ip" ]; then
        continue
    fi
    
    for dst_node in "${NODE_ARRAY[@]}"; do
        dst_type="${NODE_TYPES[$dst_node]}"
        
        # Skip if destination is not an RSU
        if [ "$dst_type" != "Rsu" ]; then
            continue
        fi
        
        if [ "$src_node" = "$dst_node" ]; then
            continue
        fi
        
        dst_ip="${NODE_IPS[$dst_node]}"
        if [ -z "$dst_ip" ]; then
            continue
        fi
        
        # Launch ping in background - all will start nearly simultaneously
        (
            pair_id="${src_node}_to_${dst_node}"
            result_file="${TEMP_DIR}/${pair_id}.txt"
            
            # Ping and extract RTT values (in ms, will convert to μs)
            # Use -i 0.05 (50ms between pings) for faster measurement
            # Use -f for flood ping if running as root (fastest, but requires root)
            ping_output=$(sudo ip netns exec "$src_ns" ping -c "$PING_COUNT" -W 1 -i 0.05 "$dst_ip" 2>/dev/null || echo "failed")
            
            if echo "$ping_output" | grep -q "rtt min/avg/max/mdev"; then
                # Extract avg RTT in ms
                avg_rtt_ms=$(echo "$ping_output" | grep "rtt min/avg/max/mdev" | awk -F'/' '{print $5}')
                # Convert to microseconds
                avg_rtt_us=$(awk -v ms="$avg_rtt_ms" 'BEGIN{printf "%.3f", ms * 1000}')
                
                # Extract all individual RTT values and convert to microseconds
                echo "$ping_output" | grep "time=" | awk '{for(i=1;i<=NF;i++) if($i~/time=/) print $i}' | sed 's/time=//' | sed 's/ms//' | while read rtt_ms; do
                    rtt_us=$(awk -v ms="$rtt_ms" 'BEGIN{printf "%.3f", ms * 1000}')
                    echo "$rtt_us"
                done > "$result_file"
                
                echo "SUCCESS|$src_node|$dst_node|$avg_rtt_us" >> "${TEMP_DIR}/pair_averages.tmp"
            else
                echo "FAILED|$src_node|$dst_node" >> "${TEMP_DIR}/failures.tmp"
            fi
        ) &
        ping_pids+=($!)
    done
done

# Wait for all pings to complete with progress indicator
total_pings=${#ping_pids[@]}
completed=0
while [ $completed -lt $total_pings ]; do
    completed=0
    for pid in "${ping_pids[@]}"; do
        if ! kill -0 $pid 2>/dev/null; then
            completed=$((completed + 1))
        fi
    done
    echo -ne "  Progress: $completed/$total_pings pings completed...\r"
    sleep 0.5
done
echo -ne "  Progress: $total_pings/$total_pings pings completed - Done!   \n"

# Collect results from temporary files
for src_node in "${NODE_ARRAY[@]}"; do
    src_type="${NODE_TYPES[$src_node]}"
    
    if [ "$src_type" != "Obu" ]; then
        continue
    fi
    
    node_latency_file="${OUTPUT_DIR}/${src_node}_latencies.txt"
    
    for dst_node in "${NODE_ARRAY[@]}"; do
        dst_type="${NODE_TYPES[$dst_node]}"
        
        if [ "$dst_type" != "Rsu" ]; then
            continue
        fi
        
        if [ "$src_node" = "$dst_node" ]; then
            continue
        fi
        
        pair_id="${src_node}_to_${dst_node}"
        result_file="${TEMP_DIR}/${pair_id}.txt"
        
        if [ -f "$result_file" ] && [ -s "$result_file" ]; then
            cat "$result_file" >> "$LATENCY_FILE"
            cat "$result_file" >> "$node_latency_file"
        fi
    done
done

# Process pair averages
if [ -f "${TEMP_DIR}/pair_averages.tmp" ]; then
    while IFS='|' read -r status src dst avg; do
        echo "$src,$dst,$avg" >> "${OUTPUT_DIR}/pair_averages.txt"
    done < "${TEMP_DIR}/pair_averages.tmp"
fi

# Process failures
if [ -f "${TEMP_DIR}/failures.tmp" ]; then
    while IFS='|' read -r status src dst; do
        echo "FAILED" >> "${OUTPUT_DIR}/failed_pairs.txt"
        echo "$src -> $dst: FAILED" >> "${OUTPUT_DIR}/failed_pairs.txt"
    done < "${TEMP_DIR}/failures.tmp"
fi

# Cleanup temporary directory
rm -rf "$TEMP_DIR"

echo ""
echo -e "${GREEN}Measurement complete!${NC}"
echo ""

# Generate histogram
if [ ! -f "$LATENCY_FILE" ] || [ ! -s "$LATENCY_FILE" ]; then
    echo "Error: No latency data collected"
    exit 1
fi

echo -e "${YELLOW}Generating histograms...${NC}"

# Calculate statistics and create histogram
cat > /tmp/latency_stats.awk << 'EOF'
BEGIN {
    min = 999999
    max = 0
    sum = 0
    count = 0
}
{
    val = $1
    if (val < min) min = val
    if (val > max) max = val
    sum += val
    count++
    latencies[count] = val
}
END {
    if (count == 0) exit
    
    avg = sum / count
    
    # Calculate median
    for (i = 1; i <= count; i++) {
        for (j = i + 1; j <= count; j++) {
            if (latencies[i] > latencies[j]) {
                tmp = latencies[i]
                latencies[i] = latencies[j]
                latencies[j] = tmp
            }
        }
    }
    median = (count % 2 == 0) ? (latencies[count/2] + latencies[count/2+1]) / 2 : latencies[(count+1)/2]
    
    # Calculate percentiles
    p50 = latencies[int(count * 0.50)]
    p90 = latencies[int(count * 0.90)]
    p95 = latencies[int(count * 0.95)]
    p99 = latencies[int(count * 0.99)]
    
    print "Statistics:"
    print "  Samples: " count
    print "  Min:     " min " μs"
    print "  Max:     " max " μs"
    print "  Average: " avg " μs"
    print "  Median:  " median " μs"
    print "  P50:     " p50 " μs"
    print "  P90:     " p90 " μs"
    print "  P95:     " p95 " μs"
    print "  P99:     " p99 " μs"
    print ""
}
EOF

# Generate global statistics and histogram
echo "=== GLOBAL STATISTICS (all nodes) ===" | tee "${OUTPUT_DIR}/statistics.txt"
echo ""
awk -f /tmp/latency_stats.awk "$LATENCY_FILE" | tee -a "${OUTPUT_DIR}/statistics.txt"

# Create global histogram with bins
echo "=== GLOBAL HISTOGRAM ===" | tee "$HISTOGRAM_FILE"
echo "" | tee -a "$HISTOGRAM_FILE"

# Determine bin size automatically (now working with microseconds)
max_latency=$(awk 'BEGIN{max=0} {if($1>max) max=$1} END{print max}' "$LATENCY_FILE")
min_latency=$(awk 'BEGIN{min=999999} {if($1<min) min=$1} END{print min}' "$LATENCY_FILE")

echo "Latency Range (μs) | Count | Distribution" | tee -a "$HISTOGRAM_FILE"
echo "-------------------|-------|-------------" | tee -a "$HISTOGRAM_FILE"

# Use adaptive logarithmic-like binning for better low-latency resolution
awk -v max_lat="$max_latency" -v min_lat="$min_latency" '
{
    latencies[NR] = $1
    total++
}
END {
    # Sort latencies
    for (i = 1; i <= total; i++) {
        for (j = i + 1; j <= total; j++) {
            if (latencies[i] > latencies[j]) {
                tmp = latencies[i]
                latencies[i] = latencies[j]
                latencies[j] = tmp
            }
        }
    }
    
        # Create adaptive bins: smaller bins for lower latencies, larger for higher
        bin_edges[0] = 0
        bin_idx = 1
        
        # Very fine bins for < 1000 μs (20 μs each)
        for (i = 20; i <= 1000; i += 20) {
            bin_edges[bin_idx++] = i
        }
        
        # Fine bins for 1-10 ms (100 μs each)
        for (i = 1100; i <= 10000; i += 100) {
            bin_edges[bin_idx++] = i
        }
        
        # Fine bins for 10-50 ms (100 μs each for better resolution)
        for (i = 10100; i <= 50000; i += 100) {
            bin_edges[bin_idx++] = i
        }
        
        # Coarser bins for > 50 ms (5000 μs each)
        for (i = 55000; i <= max_lat + 5000; i += 5000) {
            bin_edges[bin_idx++] = i
        }
        
        num_bins = bin_idx - 1    # Count samples in each bin
    for (i = 1; i <= total; i++) {
        lat = latencies[i]
        for (b = 1; b <= num_bins; b++) {
            if (lat >= bin_edges[b-1] && lat < bin_edges[b]) {
                bin_counts[b]++
                break
            }
        }
    }
    
    # Find max count for scaling
    max_count = 0
    for (b = 1; b <= num_bins; b++) {
        if (bin_counts[b] > max_count) max_count = bin_counts[b]
    }
    
    # Print histogram
    for (b = 1; b <= num_bins; b++) {
        if (bin_counts[b] > 0) {
            lower = bin_edges[b-1]
            upper = bin_edges[b]
            count = bin_counts[b]
            
            # Create bar graph (50 chars max)
            bar_length = int(count * 50 / max_count)
            bar = ""
            for (j = 0; j < bar_length; j++) bar = bar "#"
            
            printf "%8.0f - %8.0f | %5d | %s\n", lower, upper, count, bar
        }
    }
}
' "$LATENCY_FILE" | tee -a "$HISTOGRAM_FILE"

echo ""
echo ""

# Generate per-OBU histograms
echo -e "${YELLOW}Generating per-OBU histograms...${NC}"
echo ""
for src_node in "${NODE_ARRAY[@]}"; do
    # Only generate histograms for OBU nodes
    if [ "${NODE_TYPES[$src_node]}" != "Obu" ]; then
        continue
    fi
    
    node_latency_file="${OUTPUT_DIR}/${src_node}_latencies.txt"
    
    if [ ! -f "$node_latency_file" ] || [ ! -s "$node_latency_file" ]; then
        continue
    fi
    
    node_hist_file="${OUTPUT_DIR}/${src_node}_histogram.txt"
    node_stats_file="${OUTPUT_DIR}/${src_node}_statistics.txt"
    
    # Generate statistics (to file and console)
    echo "=== STATISTICS FOR $src_node (OBU -> RSU) ===" | tee "$node_stats_file"
    echo "" | tee -a "$node_stats_file"
    awk -f /tmp/latency_stats.awk "$node_latency_file" | tee -a "$node_stats_file"
    
    # Generate histogram (to file and console)
    echo "=== LATENCY HISTOGRAM FOR $src_node (OBU -> RSU) ===" | tee "$node_hist_file"
    echo "" | tee -a "$node_hist_file"
    
    # Use adaptive binning for consistency
    echo "Latency Range (μs) | Count | Distribution" | tee -a "$node_hist_file"
    echo "-------------------|-------|-------------" | tee -a "$node_hist_file"
    
    awk '
    {
        latencies[NR] = $1
        total++
        if (NR == 1) {
            max_lat = $1
            min_lat = $1
        } else {
            if ($1 > max_lat) max_lat = $1
            if ($1 < min_lat) min_lat = $1
        }
    }
    END {
        # Sort latencies
        for (i = 1; i <= total; i++) {
            for (j = i + 1; j <= total; j++) {
                if (latencies[i] > latencies[j]) {
                    tmp = latencies[i]
                    latencies[i] = latencies[j]
                    latencies[j] = tmp
                }
            }
        }
        
        # Create adaptive bins: smaller bins for lower latencies, larger for higher
        bin_edges[0] = 0
        bin_idx = 1
        
        # Very fine bins for < 1000 μs (20 μs each)
        for (i = 20; i <= 1000; i += 20) {
            bin_edges[bin_idx++] = i
        }
        
        # Fine bins for 1-10 ms (100 μs each)
        for (i = 1100; i <= 10000; i += 100) {
            bin_edges[bin_idx++] = i
        }
        
        # Fine bins for 10-50 ms (100 μs each for better resolution)
        for (i = 10100; i <= 50000; i += 100) {
            bin_edges[bin_idx++] = i
        }
        
        # Coarser bins for > 50 ms (5000 μs each)
        for (i = 55000; i <= max_lat + 5000; i += 5000) {
            bin_edges[bin_idx++] = i
        }
        
        num_bins = bin_idx - 1
        
        # Count samples in each bin
        for (i = 1; i <= total; i++) {
            lat = latencies[i]
            for (b = 1; b <= num_bins; b++) {
                if (lat >= bin_edges[b-1] && lat < bin_edges[b]) {
                    bin_counts[b]++
                    break
                }
            }
        }
        
        # Find max count for scaling
        max_count = 0
        for (b = 1; b <= num_bins; b++) {
            if (bin_counts[b] > max_count) max_count = bin_counts[b]
        }
        
        # Print histogram
        for (b = 1; b <= num_bins; b++) {
            if (bin_counts[b] > 0) {
                lower = bin_edges[b-1]
                upper = bin_edges[b]
                count = bin_counts[b]
                
                # Create bar graph (50 chars max)
                bar_length = int(count * 50 / max_count)
                bar = ""
                for (j = 0; j < bar_length; j++) bar = bar "#"
                
                printf "%8.0f - %8.0f | %5d | %s\n", lower, upper, count, bar
            }
        }
    }
    ' "$node_latency_file" | tee -a "$node_hist_file"
    
    echo ""
done

echo ""
echo -e "${GREEN}Results saved to:${NC}"
echo ""
echo -e "${BLUE}Global results:${NC}"
echo "  Histogram:     $HISTOGRAM_FILE"
echo "  Statistics:    ${OUTPUT_DIR}/statistics.txt"
echo "  Raw data:      $LATENCY_FILE"
echo "  Pair averages: ${OUTPUT_DIR}/pair_averages.txt"

echo ""
echo -e "${BLUE}Per-OBU results (OBU -> RSU):${NC}"
for src_node in "${NODE_ARRAY[@]}"; do
    if [ "${NODE_TYPES[$src_node]}" = "Obu" ] && [ -f "${OUTPUT_DIR}/${src_node}_histogram.txt" ]; then
        echo "  $src_node:"
        echo "    Histogram:  ${OUTPUT_DIR}/${src_node}_histogram.txt"
        echo "    Statistics: ${OUTPUT_DIR}/${src_node}_statistics.txt"
        echo "    Raw data:   ${OUTPUT_DIR}/${src_node}_latencies.txt"
    fi
done

if [ -f "${OUTPUT_DIR}/failed_pairs.txt" ]; then
    failed_count=$(grep -c "FAILED" "${OUTPUT_DIR}/failed_pairs.txt" 2>/dev/null || echo 0)
    if [ "$failed_count" -gt 0 ]; then
        echo ""
        echo -e "  ${YELLOW}Failed pairs: ${OUTPUT_DIR}/failed_pairs.txt ($failed_count failures)${NC}"
    fi
fi

echo ""
echo -e "${BLUE}=== Done ===${NC}"
