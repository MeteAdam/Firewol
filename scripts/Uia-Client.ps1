#Requires -Version 5.1
# Runs under Windows PowerShell, whose .NET Framework ships the UI Automation client.
# Repeatedly walks every descendant of the app window, scrolls the first scrollable element
# and counts structure and name events, writing one JSON line per iteration.
param(
    [Parameter(Mandatory)] [int] $ProcessId,
    [Parameter(Mandatory)] [int] $Seconds,
    [Parameter(Mandatory)] [string] $Output
)

$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies UIAutomationClient, UIAutomationTypes, WindowsBase -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Windows.Automation;

public static class FirewolUiaStress {
    static int events;

    public static void Run(int pid, int seconds, string output) {
        var deadline = DateTime.UtcNow.AddSeconds(seconds);
        var byProcess = new PropertyCondition(AutomationElement.ProcessIdProperty, pid);
        AutomationElement window = null;
        while (window == null && DateTime.UtcNow < deadline) {
            window = AutomationElement.RootElement.FindFirst(TreeScope.Children, byProcess);
            if (window == null) Thread.Sleep(200);
        }
        using (var log = new StreamWriter(output, false)) {
            log.AutoFlush = true;
            if (window == null) {
                log.WriteLine("{\"error\":\"window not found\"}");
                return;
            }
            Automation.AddStructureChangedEventHandler(window, TreeScope.Subtree,
                (sender, args) => Interlocked.Increment(ref events));
            Automation.AddAutomationPropertyChangedEventHandler(window, TreeScope.Subtree,
                (sender, args) => Interlocked.Increment(ref events), AutomationElement.NameProperty);
            AutomationElement scroller = null;
            double percent = 0;
            int iteration = 0;
            while (DateTime.UtcNow < deadline) {
                var watch = Stopwatch.StartNew();
                int count = -1;
                string error = null;
                try {
                    var all = window.FindAll(TreeScope.Descendants, Condition.TrueCondition);
                    count = all.Count;
                    if (scroller == null) {
                        foreach (AutomationElement element in all) {
                            object pattern;
                            if (element.TryGetCurrentPattern(ScrollPattern.Pattern, out pattern)
                                && ((ScrollPattern)pattern).Current.VerticallyScrollable) {
                                scroller = element;
                                break;
                            }
                        }
                    }
                } catch (Exception exception) {
                    error = exception.GetType().Name;
                }
                long findMs = watch.ElapsedMilliseconds;
                string scrolled = "none";
                if (scroller != null) {
                    try {
                        percent = (percent + 7) % 100;
                        var pattern = (ScrollPattern)scroller.GetCurrentPattern(ScrollPattern.Pattern);
                        pattern.SetScrollPercent(ScrollPattern.NoScroll, percent);
                        scrolled = percent.ToString("0");
                    } catch (Exception exception) {
                        scrolled = exception.GetType().Name;
                    }
                }
                log.WriteLine(string.Format(
                    "{{\"i\":{0},\"elements\":{1},\"find_ms\":{2},\"scroll\":\"{3}\",\"events\":{4},\"error\":{5}}}",
                    iteration++, count, findMs, scrolled, Thread.VolatileRead(ref events),
                    error == null ? "null" : "\"" + error + "\""));
                Thread.Sleep(250);
            }
            Automation.RemoveAllEventHandlers();
        }
    }
}
'@
[FirewolUiaStress]::Run($ProcessId, $Seconds, $Output)
