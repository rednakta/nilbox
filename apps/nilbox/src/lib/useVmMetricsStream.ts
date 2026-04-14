import { useState, useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/** Payload shape matching Rust ProxyActivitySnapshot */
interface MetricsStreamPayload {
  last_domain: string | null;
  rx_bytes_delta: number;
  tx_bytes_delta: number;
  active: boolean;
  cpu_percent: number;
  memory_used_mb: number;
  memory_total_mb: number;
  network_rx_bytes: number;
  network_tx_bytes: number;
}

/** Processed metrics exposed to React components */
export interface VmMetricsStream {
  /** Most recently accessed domain */
  lastDomain: string | null;
  /** Download bytes/sec (delta * 2 since interval is 500ms) */
  rxBytesPerSec: number;
  /** Upload bytes/sec */
  txBytesPerSec: number;
  /** Whether network traffic is active in this interval */
  networkActive: boolean;
  /** CPU % (cached, updated every 15s) */
  cpuPercent: number;
  /** Memory used MB */
  memoryUsedMb: number;
  /** Memory total MB */
  memoryTotalMb: number;
  /** Cumulative download bytes */
  networkRxBytes: number;
  /** Cumulative upload bytes */
  networkTxBytes: number;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

const IDLE_TIMEOUT_MS = 3000;

const defaultMetrics: VmMetricsStream = {
  lastDomain: null,
  rxBytesPerSec: 0,
  txBytesPerSec: 0,
  networkActive: false,
  cpuPercent: 0,
  memoryUsedMb: 0,
  memoryTotalMb: 0,
  networkRxBytes: 0,
  networkTxBytes: 0,
};

export function useVmMetricsStream(): VmMetricsStream {
  const [metrics, setMetrics] = useState<VmMetricsStream>(defaultMetrics);
  const idleTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    let unlisten: (() => void) | null = null;

    listen<MetricsStreamPayload>("vm-metrics-stream", (event) => {
      const d = event.payload;

      if (d.active) {
        // Active traffic: show network info immediately, reset idle timer
        clearTimeout(idleTimer.current);
        setMetrics({
          lastDomain: d.last_domain,
          rxBytesPerSec: d.rx_bytes_delta * 2,
          txBytesPerSec: d.tx_bytes_delta * 2,
          networkActive: true,
          cpuPercent: d.cpu_percent,
          memoryUsedMb: d.memory_used_mb,
          memoryTotalMb: d.memory_total_mb,
          networkRxBytes: d.network_rx_bytes,
          networkTxBytes: d.network_tx_bytes,
        });
        idleTimer.current = setTimeout(() => {
          setMetrics((prev) => ({
            ...prev, networkActive: false, lastDomain: null,
            rxBytesPerSec: 0, txBytesPerSec: 0,
          }));
        }, IDLE_TIMEOUT_MS);
      } else {
        // Inactive: update CPU/memory only, keep last network display until idle timer fires
        setMetrics((prev) => ({
          ...prev,
          cpuPercent: d.cpu_percent,
          memoryUsedMb: d.memory_used_mb,
          memoryTotalMb: d.memory_total_mb,
          networkRxBytes: d.network_rx_bytes,
          networkTxBytes: d.network_tx_bytes,
        }));
      }
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
      clearTimeout(idleTimer.current);
    };
  }, []);

  return metrics;
}
