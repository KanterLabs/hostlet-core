use super::*;
use hostlet_contracts::HostResourceSnapshot;

pub(crate) async fn collect_host_resource_snapshot() -> anyhow::Result<HostResourceSnapshot> {
    let memory = tokio::fs::read_to_string("/proc/meminfo").await?;
    let memory_total_mib = meminfo_kib(&memory, "MemTotal").unwrap_or(0) / 1024;
    let memory_available_mib = meminfo_kib(&memory, "MemAvailable").unwrap_or(0) / 1024;
    let swap_total_mib = meminfo_kib(&memory, "SwapTotal").unwrap_or(0) / 1024;
    let swap_free_mib = meminfo_kib(&memory, "SwapFree").unwrap_or(0) / 1024;

    let load = tokio::fs::read_to_string("/proc/loadavg").await?;
    let mut load_fields = load.split_whitespace();
    let load_one = parse_load(load_fields.next());
    let load_five = parse_load(load_fields.next());
    let load_fifteen = parse_load(load_fields.next());

    let disk_path = std::env::var("HOSTLET_RESOURCE_DISK_PATH")
        .ok()
        .filter(|path| Path::new(path).exists())
        .unwrap_or_else(|| "/".to_string());
    let disk = command_output("df", &["-Pk", &disk_path], Duration::from_secs(10)).await?;
    let (disk_total_mib, disk_free_mib) = parse_df_kib(&disk.stdout)?;

    let containers = command_output(
        "docker",
        &["ps", "--format", "{{.Names}}"],
        Duration::from_secs(10),
    )
    .await?;
    let running_containers = if containers.status.success() {
        String::from_utf8(containers.stdout)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    } else {
        0
    };

    Ok(HostResourceSnapshot {
        memory_total_mib,
        memory_available_mib,
        swap_used_mib: swap_total_mib.saturating_sub(swap_free_mib),
        disk_total_mib,
        disk_free_mib,
        load_one,
        load_five,
        load_fifteen,
        running_containers,
    })
}

fn meminfo_kib(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key).then(|| value.split_whitespace().next()?.parse().ok())?
    })
}

fn parse_load(value: Option<&str>) -> f64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0.0)
}

fn parse_df_kib(stdout: &[u8]) -> anyhow::Result<(u64, u64)> {
    let output = String::from_utf8(stdout.to_vec())?;
    let line = output
        .lines()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("df did not return a filesystem row"))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 4 {
        bail!("df returned an incomplete filesystem row");
    }
    let total_kib: u64 = fields[1].parse()?;
    let free_kib: u64 = fields[3].parse()?;
    Ok((total_kib / 1024, free_kib / 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_values() {
        let input = "MemTotal:       8192000 kB\nMemAvailable:   6144000 kB\n";
        assert_eq!(meminfo_kib(input, "MemTotal"), Some(8_192_000));
        assert_eq!(meminfo_kib(input, "MemAvailable"), Some(6_144_000));
    }

    #[test]
    fn parses_posix_df_output() {
        let input = b"Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/vda 209715200 1 104857600 50% /\n";
        assert_eq!(parse_df_kib(input).unwrap(), (204_800, 102_400));
    }
}
