use super::*;

const DOCKER_METRIC_MAX_BYTES: f64 = 1_125_899_906_842_624.0;
const DOCKER_METRIC_MAX_COUNT: i64 = 1_000_000;
const DOCKER_METRIC_MAX_PERCENT: f64 = 1_000_000.0;

pub(crate) async fn publish_resource_stats(cfg: &Config) {
    let Ok(containers) = hostlet_containers().await else {
        return;
    };
    if containers.is_empty() {
        return;
    }
    let mut args = vec!["stats", "--no-stream", "--format", "json"];
    args.extend(containers.iter().map(String::as_str));
    let Ok(output) = command_output("docker", &args, Duration::from_secs(15)).await else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(container) = raw
            .get("Container")
            .or_else(|| raw.get("Name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if !valid_container_name(container) {
            continue;
        }
        let cpu_percent = raw.get("CPUPerc").and_then(|v| v.as_str()).unwrap_or("0%");
        let memory_usage = raw
            .get("MemUsage")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let memory_percent = raw.get("MemPerc").and_then(|v| v.as_str()).unwrap_or("0%");
        let network_io = raw
            .get("NetIO")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let block_io = raw
            .get("BlockIO")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let pids = raw.get("PIDs").and_then(|v| v.as_str()).unwrap_or("0");
        let (memory_usage_bytes, memory_limit_bytes) = parse_docker_byte_pair(memory_usage);
        let (network_rx_bytes, network_tx_bytes) = parse_docker_byte_pair(network_io);
        let (block_read_bytes, block_write_bytes) = parse_docker_byte_pair(block_io);
        post(
            cfg,
            json!({
                "type": "resource_stats",
                "container": container,
                "cpuPercent": cpu_percent,
                "cpuPercentValue": parse_percent(cpu_percent),
                "memoryUsage": memory_usage,
                "memoryUsageBytes": memory_usage_bytes,
                "memoryLimitBytes": memory_limit_bytes,
                "memoryPercent": memory_percent,
                "memoryPercentValue": parse_percent(memory_percent),
                "networkIo": network_io,
                "networkRxBytes": network_rx_bytes,
                "networkTxBytes": network_tx_bytes,
                "blockIo": block_io,
                "blockReadBytes": block_read_bytes,
                "blockWriteBytes": block_write_bytes,
                "pids": pids,
                "pidsCurrent": parse_metric_count(pids)
            }),
        )
        .await;
    }
}

fn parse_percent(value: &str) -> Option<f64> {
    let percent = value.trim().trim_end_matches('%').parse::<f64>().ok()?;
    (percent.is_finite() && (0.0..=DOCKER_METRIC_MAX_PERCENT).contains(&percent)).then_some(percent)
}

fn parse_docker_byte_pair(value: &str) -> (Option<i64>, Option<i64>) {
    let mut parts = value.split('/').map(str::trim);
    let first = parts.next().and_then(parse_docker_bytes);
    let second = parts.next().and_then(parse_docker_bytes);
    (first, second)
}

fn parse_metric_count(value: &str) -> Option<i64> {
    let count = value.trim().parse::<i64>().ok()?;
    (0..=DOCKER_METRIC_MAX_COUNT)
        .contains(&count)
        .then_some(count)
}

fn parse_docker_bytes(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let number_len = value
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .last()
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);
    if number_len == 0 {
        return None;
    }
    let number = value[..number_len].parse::<f64>().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = value[number_len..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1.0,
        "kb" => 1_000.0,
        "kib" => 1024.0,
        "mb" => 1_000_000.0,
        "mib" => 1024.0 * 1024.0,
        "gb" => 1_000_000_000.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" => 1_000_000_000_000.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = (number * multiplier).round();
    (bytes.is_finite() && (0.0..=DOCKER_METRIC_MAX_BYTES).contains(&bytes)).then_some(bytes as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_resource_stats_are_parseable_for_budget_checks() {
        assert_eq!(parse_percent("12.5%"), Some(12.5));
        assert_eq!(parse_docker_bytes("1.5MiB"), Some(1_572_864));
        assert_eq!(parse_docker_bytes("2kB"), Some(2_000));
        assert_eq!(parse_docker_bytes("1TiB"), Some(1_099_511_627_776));
        assert_eq!(parse_metric_count("7"), Some(7));
        assert_eq!(
            parse_docker_byte_pair("12.5MiB / 1GiB"),
            (Some(13_107_200), Some(1_073_741_824))
        );
        assert_eq!(parse_docker_byte_pair("1.2kB / 0B"), (Some(1_200), Some(0)));
    }

    #[test]
    fn docker_resource_stats_reject_invalid_numeric_values() {
        assert_eq!(parse_percent("-1%"), None);
        assert_eq!(parse_percent("NaN%"), None);
        assert_eq!(parse_percent("inf%"), None);
        assert_eq!(parse_percent("1000001%"), None);

        assert_eq!(parse_docker_bytes("-1B"), None);
        assert_eq!(parse_docker_bytes("NaNB"), None);
        assert_eq!(parse_docker_bytes("2PiB"), None);
        assert_eq!(parse_docker_bytes("1125899906842625B"), None);
        assert_eq!(parse_metric_count("-1"), None);
        assert_eq!(parse_metric_count("1000001"), None);
        assert_eq!(parse_docker_byte_pair("NaNB / 2PiB"), (None, None));
    }
}
