use super::*;
use hostlet_contracts::{HostResourceSnapshot, MAX_LOGICAL_CPU_COUNT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// Retains the previous aggregate `/proc/stat` sample across heartbeats.
/// CPU utilization is only meaningful as a delta, so the first sample is
/// intentionally reported as unavailable.
#[derive(Default)]
pub(crate) struct HostResourceSampler {
    previous_cpu: Option<CpuTimes>,
}

impl HostResourceSampler {
    fn cpu_utilization_percent(&mut self, proc_stat: &str) -> Option<f64> {
        let current = cpu_times_from_proc_stat(proc_stat)?;
        let utilization = self.previous_cpu.and_then(|previous| {
            let total_delta = current.total.checked_sub(previous.total)?;
            let idle_delta = current.idle.checked_sub(previous.idle)?;
            if total_delta == 0 || idle_delta > total_delta {
                return None;
            }
            Some((100.0 * (total_delta - idle_delta) as f64 / total_delta as f64).clamp(0.0, 100.0))
        });
        self.previous_cpu = Some(current);
        utilization
    }
}

pub(crate) async fn collect_host_resource_snapshot(
    sampler: &mut HostResourceSampler,
) -> anyhow::Result<HostResourceSnapshot> {
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

    // CPU telemetry is deliberately best-effort: a transient unreadable proc
    // file must not suppress memory, disk, load, or container telemetry.
    let cpu_utilization_percent = tokio::fs::read_to_string("/proc/stat")
        .await
        .ok()
        .and_then(|proc_stat| sampler.cpu_utilization_percent(&proc_stat));
    let logical_cpu_count = tokio::fs::read_to_string("/proc/cpuinfo")
        .await
        .ok()
        .and_then(|cpuinfo| logical_cpu_count_from_cpuinfo(&cpuinfo))
        .or_else(logical_cpu_count_from_available_parallelism);

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
        cpu_utilization_percent,
        logical_cpu_count,
    })
}

/// Returns aggregate CPU times from the leading `cpu` record in `/proc/stat`.
/// Linux already includes guest and guest_nice time in user and nice, so the
/// final two guest fields are intentionally excluded from `total`.
fn cpu_times_from_proc_stat(contents: &str) -> Option<CpuTimes> {
    let line = contents
        .lines()
        .find(|line| line.split_whitespace().next() == Some("cpu"))?;
    let values = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values
        .iter()
        .try_fold(0_u64, |total, value| total.checked_add(*value))?;
    let idle = values[3].checked_add(*values.get(4).unwrap_or(&0))?;
    Some(CpuTimes { total, idle })
}

fn logical_cpu_count_from_cpuinfo(contents: &str) -> Option<u32> {
    let count = contents
        .lines()
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(key, _)| key.trim() == "processor")
        })
        .count();
    u32::try_from(count)
        .ok()
        .filter(|count| (1..=MAX_LOGICAL_CPU_COUNT).contains(count))
}

fn logical_cpu_count_from_available_parallelism() -> Option<u32> {
    std::thread::available_parallelism()
        .ok()
        .and_then(|count| u32::try_from(count.get()).ok())
        .filter(|count| (1..=MAX_LOGICAL_CPU_COUNT).contains(count))
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

    #[test]
    fn cpu_utilization_uses_proc_stat_deltas_without_guest_double_counting() {
        let mut sampler = HostResourceSampler::default();
        // guest and guest_nice are present but must not contribute separately
        // to total time (their values are already included in user/nice).
        assert_eq!(
            sampler.cpu_utilization_percent("cpu  100 20 30 800 50 10 5 5 40 10\n"),
            None
        );
        assert_eq!(
            sampler.cpu_utilization_percent("cpu  150 20 30 850 50 10 5 5 9999 999\n"),
            Some(50.0)
        );
    }

    #[test]
    fn cpu_utilization_rejects_counter_resets_and_zero_deltas() {
        let mut sampler = HostResourceSampler::default();
        assert_eq!(sampler.cpu_utilization_percent("cpu  1 1 1 1\n"), None);
        assert_eq!(sampler.cpu_utilization_percent("cpu  1 1 1 1\n"), None);
        assert_eq!(sampler.cpu_utilization_percent("cpu  0 1 1 1\n"), None);
    }

    #[test]
    fn parses_logical_cpu_count_from_cpuinfo() {
        let cpuinfo = "processor : 0\nmodel name : test\n\nprocessor : 1\n";
        assert_eq!(logical_cpu_count_from_cpuinfo(cpuinfo), Some(2));
        assert_eq!(logical_cpu_count_from_cpuinfo("model name : test\n"), None);
        assert_eq!(
            logical_cpu_count_from_cpuinfo(
                &"processor : 0\n".repeat((MAX_LOGICAL_CPU_COUNT + 1) as usize)
            ),
            None
        );
    }
}
