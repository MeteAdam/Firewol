if (-not ('FirewolM0Memory' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class FirewolM0Memory {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct ProcessEntry {
        public uint Size, Usage, Id;
        public UIntPtr HeapId;
        public uint ModuleId, Threads, ParentId;
        public int Priority;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)]
        public string Exe;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MemoryCounters {
        public uint Size, Faults;
        public UIntPtr PeakWorkingSet, WorkingSet, PeakPaged, Paged, PeakNonPaged, NonPaged;
        public UIntPtr Pagefile, PeakPagefile, PrivateCommit, PrivateWorkingSet;
        public ulong SharedCommit;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint id);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode, ExactSpelling = true)]
    static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode, ExactSpelling = true)]
    static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool K32GetProcessMemoryInfo(IntPtr process, ref MemoryCounters counters, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);

    public static ulong Measure(uint[] roots) {
        var children = new Dictionary<uint, List<uint>>();
        IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == new IntPtr(-1)) throw new Win32Exception();
        try {
            var entry = new ProcessEntry { Size = (uint)Marshal.SizeOf(typeof(ProcessEntry)) };
            if (!Process32FirstW(snapshot, ref entry)) throw new Win32Exception();
            do {
                List<uint> siblings;
                if (!children.TryGetValue(entry.ParentId, out siblings)) {
                    siblings = new List<uint>();
                    children.Add(entry.ParentId, siblings);
                }
                siblings.Add(entry.Id);
            } while (Process32NextW(snapshot, ref entry));
        } finally {
            CloseHandle(snapshot);
        }

        ulong bytes = 0;
        var pending = new Queue<uint>(roots);
        var seen = new HashSet<uint>();
        while (pending.Count > 0) {
            uint id = pending.Dequeue();
            if (!seen.Add(id)) continue;
            bytes += Read(id);
            List<uint> descendants;
            if (children.TryGetValue(id, out descendants)) {
                foreach (uint child in descendants) pending.Enqueue(child);
            }
        }
        return bytes;
    }

    static ulong Read(uint id) {
        const uint queryLimitedInformation = 0x1000;
        IntPtr process = OpenProcess(queryLimitedInformation, false, id);
        if (process == IntPtr.Zero) {
            int error = Marshal.GetLastWin32Error();
            if (error == 87) return 0; // Process exited since the snapshot.
            throw new Win32Exception(error, "Cannot open measured process " + id);
        }
        try {
            var counters = new MemoryCounters { Size = (uint)Marshal.SizeOf(typeof(MemoryCounters)) };
            if (!K32GetProcessMemoryInfo(process, ref counters, counters.Size)) {
                int error = Marshal.GetLastWin32Error();
                uint exitCode;
                if (GetExitCodeProcess(process, out exitCode) && exitCode != 259) return 0;
                throw new Win32Exception(error, "Cannot read private working set for " + id);
            }
            return counters.PrivateWorkingSet.ToUInt64();
        } finally {
            CloseHandle(process);
        }
    }
}
'@
}
