"""Linux per-process memory observations, never whole-device use as child allocation."""
from pathlib import Path
import subprocess
import threading
import time

class MemorySampler:
    def __init__(self, pid, interval=0.1):
        self.pid = pid
        self.interval = interval
        self.samples = []
        self.errors = set()
        self.stop_event = threading.Event()
        self.thread = threading.Thread(target=self._sample, daemon=True)

    def start(self):
        self.thread.start()
        return self

    def _sample(self):
        while not self.stop_event.is_set():
            sample = {'monotonic_seconds': time.monotonic(), 'rss_kib': None, 'gpu_mib': None}
            try:
                for line in Path(f'/proc/{self.pid}/status').read_text().splitlines():
                    if line.startswith('VmRSS:'):
                        sample['rss_kib'] = int(line.split()[1])
            except (OSError, ValueError) as error:
                self.errors.add('RSS unavailable: ' + type(error).__name__)
            try:
                query = subprocess.run(['nvidia-smi', '--query-compute-apps=pid,used_memory',
                                        '--format=csv,noheader,nounits'],
                                       capture_output=True, text=True, timeout=3)
                if query.returncode:
                    self.errors.add('nvidia-smi query failed')
                else:
                    matching = []
                    for row in query.stdout.splitlines():
                        fields = row.split(',')
                        if len(fields) == 2 and fields[0].strip() == str(self.pid):
                            matching.append(int(fields[1].strip()))
                    # No row does not establish zero allocation: no CUDA context may exist yet.
                    if matching:
                        sample['gpu_mib'] = sum(matching)
            except (OSError, ValueError, subprocess.TimeoutExpired) as error:
                self.errors.add('GPU allocation unavailable: ' + type(error).__name__)
            self.samples.append(sample)
            self.stop_event.wait(self.interval)

    def finish(self):
        self.stop_event.set()
        self.thread.join(timeout=4)
        def maximum(key):
            values = [s[key] for s in self.samples if s[key] is not None]
            return max(values) if values else None
        return {'peak_rss_kib': maximum('rss_kib'), 'peak_child_gpu_mib': maximum('gpu_mib'),
                'samples': self.samples, 'errors': sorted(self.errors),
                'limitations': 'Sampled peaks may miss transients; GPU memory is NVML compute-process attribution for this PID only; absent rows are unknown, not zero. External GPU use may affect throughput.'}
