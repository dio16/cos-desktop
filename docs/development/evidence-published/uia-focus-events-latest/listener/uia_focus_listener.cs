// Test-only UI Automation focus-event listener for the native AX audit.
//
// Subscribes to the UIA focus-changed event (UIA_AutomationFocusChangedEventId)
// as an ordinary same-integrity client, records every event with a timestamp,
// runtime id, name, control type, class, automation id, process id and
// keyboard-focus facts, and independently polls AutomationElement.FocusedElement
// so the focused element after each input step is observable even when no event
// fires. It is a measurement instrument: it never sends input, never elevates
// and never writes into the product.
//
// Build (Windows PowerShell 5.1 / .NET Framework 4.x csc):
//   csc.exe -nologo -out:uia_focus_listener.exe uia_focus_listener.cs
//         -r:UIAutomationClient.dll -r:UIAutomationTypes.dll -r:WindowsBase.dll
// (the driver resolves those assemblies from the GAC).
//
// Usage:
//   uia_focus_listener.exe --out <events.jsonl> --summary <summary.json>
//       --status <status.json> --target-pid <pid> --seconds 180
//       [--poll-ms 100] [--stop-file <path>] [--max-events 20000]
//       [--no-poll] [--label <text>]
//
// Exit code 0 when the listener ran and wrote its summary; 2 on a setup error.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Automation;

public static class UiaFocusListener
{
    [DllImport("user32.dll")]
    private static extern bool PeekMessage(out MSG msg, IntPtr hwnd, uint min, uint max, uint remove);
    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref MSG msg);
    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessage(ref MSG msg);

    [StructLayout(LayoutKind.Sequential)]
    public struct MSG
    {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public int ptX;
        public int ptY;
    }

    private const uint PM_REMOVE = 0x0001;

    private static readonly object Gate = new object();
    private static StreamWriter writer;
    private static string outPath = "";
    private static string summaryPath = "";
    private static string statusPath = "";
    private static string stopFile = "";
    private static string label = "";
    private static int targetPid;
    private static int pollMs = 100;
    private static int seconds = 120;
    private static int maxEvents = 20000;
    private static bool pollEnabled = true;
    private static int eventCount;
    private static int pollCount;
    private static long seq;
    private static DateTime startUtc;
    private static Stopwatch clock;
    private static string subscribeError = "";
    private static bool subscribed;
    private static AutomationFocusChangedEventHandler handler;
    private static string lastPollRid = "\u0001";
    private static bool lastPollNull = true;
    private static DateTime? firstEventUtc;
    private static DateTime? lastEventUtc;
    private static string stopReason = "";
    private static readonly Dictionary<string, string> targetNames = new Dictionary<string, string>();
    private static readonly Dictionary<string, int> targetCounts = new Dictionary<string, int>();
    private static volatile bool stopRequested;

    public static int Main(string[] args)
    {
        if (!ParseArgs(args))
        {
            Console.Error.WriteLine("usage: uia_focus_listener --out <jsonl> --summary <json> --status <json> --target-pid <pid> [--seconds N] [--poll-ms N] [--stop-file <path>] [--max-events N] [--no-poll] [--label text]");
            return 2;
        }
        try
        {
            string directory = Path.GetDirectoryName(Path.GetFullPath(outPath));
            if (!string.IsNullOrEmpty(directory)) { Directory.CreateDirectory(directory); }
            writer = new StreamWriter(new FileStream(outPath, FileMode.Create, FileAccess.Write, FileShare.Read), new UTF8Encoding(false));
            // Flush every record: the driver reads this log live to attribute
            // each input step and to detect a foreign window stealing focus.
            writer.AutoFlush = true;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine("cannot open the event log: " + error.Message);
            return 2;
        }

        startUtc = DateTime.UtcNow;
        clock = Stopwatch.StartNew();
        WriteStatus("starting");

        handler = new AutomationFocusChangedEventHandler(OnFocus);
        try
        {
            Automation.AddAutomationFocusChangedEventHandler(handler);
            subscribed = true;
        }
        catch (Exception error)
        {
            subscribeError = error.GetType().Name + ": " + error.Message;
        }
        WriteStatus("subscribed");

        try { Append(Snapshot(AutomationElement.FocusedElement, "start", 0, "")); }
        catch (Exception error) { RecordError("start", error); }

        DateTime deadline = startUtc.AddSeconds(seconds);
        MSG message;
        while (true)
        {
            while (PeekMessage(out message, IntPtr.Zero, 0, 0, PM_REMOVE))
            {
                TranslateMessage(ref message);
                DispatchMessage(ref message);
            }
            if (stopRequested) { stopReason = "max_events"; break; }
            if (DateTime.UtcNow >= deadline) { stopReason = "duration"; break; }
            if (stopFile.Length > 0 && File.Exists(stopFile)) { stopReason = "stop_file"; break; }
            if (pollEnabled) { PollFocused(); }
            Thread.Sleep(Math.Max(10, pollMs));
        }

        if (subscribed)
        {
            try { Automation.RemoveAutomationFocusChangedEventHandler(handler); }
            catch (Exception) { }
        }
        lock (Gate)
        {
            if (writer != null) { try { writer.Flush(); writer.Dispose(); } catch (Exception) { } writer = null; }
        }
        WriteSummary();
        WriteStatus("done");
        return 0;
    }

    // -- arguments -------------------------------------------------------------
    private static bool ParseArgs(string[] args)
    {
        for (int index = 0; index < args.Length; index++)
        {
            string name = args[index];
            string value = index + 1 < args.Length ? args[index + 1] : "";
            switch (name)
            {
                case "--out": outPath = value; index++; break;
                case "--summary": summaryPath = value; index++; break;
                case "--status": statusPath = value; index++; break;
                case "--stop-file": stopFile = value; index++; break;
                case "--label": label = value; index++; break;
                case "--target-pid": if (!int.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out targetPid)) { return false; } index++; break;
                case "--seconds": if (!int.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out seconds)) { return false; } index++; break;
                case "--poll-ms": if (!int.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out pollMs)) { return false; } index++; break;
                case "--max-events": if (!int.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out maxEvents)) { return false; } index++; break;
                case "--no-poll": pollEnabled = false; break;
                default: return false;
            }
        }
        return outPath.Length > 0 && summaryPath.Length > 0 && statusPath.Length > 0;
    }

    // -- event path ------------------------------------------------------------
    private static void OnFocus(object sender, AutomationFocusChangedEventArgs args)
    {
        try
        {
            AutomationElement element = sender as AutomationElement;
            int eventId = 0;
            try { if (args != null && args.EventId != null) { eventId = args.EventId.Id; } } catch (Exception) { }
            FocusRecord record = Snapshot(element, "event", eventId, "");
            try
            {
                AutomationElement focused = AutomationElement.FocusedElement;
                if (focused != null)
                {
                    record.focused_runtime_id = RuntimeId(focused);
                    record.focused_name = GetName(focused);
                    record.focused_process_id = GetProcessId(focused);
                }
                else { record.focused_runtime_id = ""; record.focused_name = ""; record.focused_process_id = 0; }
            }
            catch (Exception error)
            {
                record.note = JoinNote(record.note, "focused_element_error=" + error.GetType().Name);
            }
            Append(record);
        }
        catch (Exception)
        {
            // An event handler must never take the listener down.
        }
    }

    private static void PollFocused()
    {
        try
        {
            AutomationElement focused = AutomationElement.FocusedElement;
            if (focused == null)
            {
                if (!lastPollNull)
                {
                    lastPollNull = true;
                    lastPollRid = "";
                    FocusRecord nullRecord = NewRecord("poll", 0, "focused_element_null");
                    Append(nullRecord);
                }
                return;
            }
            string rid = RuntimeId(focused);
            if (!lastPollNull && rid == lastPollRid) { return; }
            lastPollNull = false;
            lastPollRid = rid;
            Append(Snapshot(focused, "poll", 0, "focused_element_changed"));
        }
        catch (Exception)
        {
            // FocusedElement can be transiently unavailable while a window is closing.
        }
    }

    private static FocusRecord NewRecord(string kind, int eventId, string note)
    {
        FocusRecord record = new FocusRecord();
        record.kind = kind;
        record.event_id = eventId;
        record.note = note == null ? "" : note;
        record.utc = DateTime.UtcNow;
        record.local = DateTimeOffset.Now;
        record.since_start_ms = clock.ElapsedMilliseconds;
        return record;
    }

    private static FocusRecord Snapshot(AutomationElement element, string kind, int eventId, string note)
    {
        FocusRecord record = NewRecord(kind, eventId, note);
        if (element == null) { return record; }
        record.has_element = true;
        record.runtime_id = RuntimeId(element);
        record.name = GetName(element);
        record.control_type = GetControlType(element);
        record.class_name = GetClassName(element);
        record.automation_id = GetAutomationId(element);
        record.process_id = GetProcessId(element);
        record.keyboard_focusable = GetKeyboardFocusable(element);
        record.enabled = GetEnabled(element);
        record.has_keyboard_focus = GetHasKeyboardFocus(element);
        record.bounds = GetBounds(element);
        record.focused_runtime_id = record.runtime_id;
        record.focused_name = record.name;
        record.focused_process_id = record.process_id;
        return record;
    }

    private static void RecordError(string kind, Exception error)
    {
        FocusRecord record = NewRecord(kind, 0, "error=" + error.GetType().Name + ": " + error.Message);
        Append(record);
    }

    private static void Append(FocusRecord record)
    {
        lock (Gate)
        {
            if (writer == null) { return; }
            seq++;
            record.seq = seq;
            record.label = label;
            try
            {
                writer.WriteLine(record.ToJson());
            }
            catch (Exception) { }
            if (record.kind == "event")
            {
                eventCount++;
                if (firstEventUtc == null) { firstEventUtc = record.utc; }
                lastEventUtc = record.utc;
            }
            else if (record.kind == "poll") { pollCount++; }
            if (record.process_id == targetPid && record.runtime_id.Length > 0)
            {
                targetNames[record.runtime_id] = record.name;
                int count;
                targetCounts.TryGetValue(record.runtime_id, out count);
                targetCounts[record.runtime_id] = count + 1;
            }
            if (seq >= maxEvents) { stopRequested = true; }
        }
    }

    // -- element facts ---------------------------------------------------------
    private static string RuntimeId(AutomationElement element)
    {
        try
        {
            int[] id = element.GetRuntimeId();
            if (id == null || id.Length == 0) { return ""; }
            StringBuilder text = new StringBuilder();
            for (int index = 0; index < id.Length; index++)
            {
                if (index > 0) { text.Append('.'); }
                text.Append(id[index].ToString(CultureInfo.InvariantCulture));
            }
            return text.ToString();
        }
        catch (Exception) { return ""; }
    }

    private static string GetName(AutomationElement element) { try { return element.Current.Name ?? ""; } catch (Exception) { return ""; } }
    private static string GetControlType(AutomationElement element) { try { return element.Current.ControlType == null ? "" : element.Current.ControlType.ProgrammaticName; } catch (Exception) { return ""; } }
    private static string GetClassName(AutomationElement element) { try { return element.Current.ClassName ?? ""; } catch (Exception) { return ""; } }
    private static string GetAutomationId(AutomationElement element) { try { return element.Current.AutomationId ?? ""; } catch (Exception) { return ""; } }
    private static int GetProcessId(AutomationElement element) { try { return element.Current.ProcessId; } catch (Exception) { return 0; } }
    private static bool? GetKeyboardFocusable(AutomationElement element) { try { return element.Current.IsKeyboardFocusable; } catch (Exception) { return null; } }
    private static bool? GetEnabled(AutomationElement element) { try { return element.Current.IsEnabled; } catch (Exception) { return null; } }
    private static bool? GetHasKeyboardFocus(AutomationElement element) { try { return element.Current.HasKeyboardFocus; } catch (Exception) { return null; } }
    private static double[] GetBounds(AutomationElement element)
    {
        try
        {
            System.Windows.Rect rect = element.Current.BoundingRectangle;
            if (rect.IsEmpty) { return null; }
            return new double[] { rect.Left, rect.Top, rect.Width, rect.Height };
        }
        catch (Exception) { return null; }
    }

    private static string JoinNote(string existing, string extra)
    {
        if (string.IsNullOrEmpty(existing)) { return extra; }
        return existing + "; " + extra;
    }

    // -- output ----------------------------------------------------------------
    private static string NowUtc() { return DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture); }

    private static void WriteStatus(string state)
    {
        if (statusPath.Length == 0) { return; }
        try
        {
            StringBuilder json = new StringBuilder();
            json.Append('{');
            json.Append("\"state\":").Append(J(state)).Append(',');
            json.Append("\"subscribed\":").Append(subscribed ? "true" : "false").Append(',');
            json.Append("\"subscribe_error\":").Append(J(subscribeError)).Append(',');
            json.Append("\"listener_pid\":").Append(Process.GetCurrentProcess().Id).Append(',');
            json.Append("\"target_pid\":").Append(targetPid).Append(',');
            json.Append("\"started_utc\":").Append(J(startUtc.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"poll_ms\":").Append(pollMs);
            json.Append('}');
            WriteAtomic(statusPath, json.ToString());
        }
        catch (Exception) { }
    }

    private static void WriteSummary()
    {
        try
        {
            StringBuilder json = new StringBuilder();
            json.Append('{');
            json.Append("\"schema\":\"uia-focus-listener/1\",");
            json.Append("\"label\":").Append(J(label)).Append(',');
            json.Append("\"listener_pid\":").Append(Process.GetCurrentProcess().Id).Append(',');
            json.Append("\"target_pid\":").Append(targetPid).Append(',');
            json.Append("\"subscribed\":").Append(subscribed ? "true" : "false").Append(',');
            json.Append("\"subscribe_error\":").Append(J(subscribeError)).Append(',');
            json.Append("\"poll_enabled\":").Append(pollEnabled ? "true" : "false").Append(',');
            json.Append("\"poll_ms\":").Append(pollMs).Append(',');
            json.Append("\"started_utc\":").Append(J(startUtc.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"ended_utc\":").Append(J(NowUtc())).Append(',');
            json.Append("\"duration_ms\":").Append(clock.ElapsedMilliseconds).Append(',');
            json.Append("\"stop_reason\":").Append(J(stopReason)).Append(',');
            json.Append("\"event_count\":").Append(eventCount).Append(',');
            json.Append("\"poll_count\":").Append(pollCount).Append(',');
            json.Append("\"record_count\":").Append(seq).Append(',');
            json.Append("\"first_event_utc\":").Append(firstEventUtc == null ? "null" : J(firstEventUtc.Value.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"last_event_utc\":").Append(lastEventUtc == null ? "null" : J(lastEventUtc.Value.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"target_elements\":[");
            bool first = true;
            foreach (KeyValuePair<string, int> pair in targetCounts)
            {
                if (!first) { json.Append(','); }
                first = false;
                json.Append('{');
                json.Append("\"runtime_id\":").Append(J(pair.Key)).Append(',');
                json.Append("\"name\":").Append(J(targetNames[pair.Key])).Append(',');
                json.Append("\"records\":").Append(pair.Value);
                json.Append('}');
            }
            json.Append("]}");
            WriteAtomic(summaryPath, json.ToString());
        }
        catch (Exception) { }
    }

    private static void WriteAtomic(string path, string content)
    {
        string temporary = path + ".tmp";
        File.WriteAllText(temporary, content, new UTF8Encoding(false));
        if (File.Exists(path)) { File.Delete(path); }
        File.Move(temporary, path);
    }

    private static string J(string value)
    {
        if (value == null) { return "null"; }
        StringBuilder text = new StringBuilder(value.Length + 2);
        text.Append('"');
        foreach (char character in value)
        {
            switch (character)
            {
                case '"': text.Append("\\\""); break;
                case '\\': text.Append("\\\\"); break;
                case '\n': text.Append("\\n"); break;
                case '\r': text.Append("\\r"); break;
                case '\t': text.Append("\\t"); break;
                default:
                    if (character < 0x20) { text.Append("\\u").Append(((int)character).ToString("x4", CultureInfo.InvariantCulture)); }
                    else { text.Append(character); }
                    break;
            }
        }
        text.Append('"');
        return text.ToString();
    }

    private static string Jb(bool? value)
    {
        if (value == null) { return "null"; }
        return value.Value ? "true" : "false";
    }

    // -- record ----------------------------------------------------------------
    private sealed class FocusRecord
    {
        public long seq;
        public string kind = "";
        public string label = "";
        public DateTime utc;
        public DateTimeOffset local;
        public long since_start_ms;
        public int event_id;
        public bool has_element;
        public string runtime_id = "";
        public string name = "";
        public string control_type = "";
        public string class_name = "";
        public string automation_id = "";
        public int process_id;
        public bool? keyboard_focusable;
        public bool? enabled;
        public bool? has_keyboard_focus;
        public double[] bounds;
        public string focused_runtime_id = "";
        public string focused_name = "";
        public int focused_process_id;
        public string note = "";

        public string ToJson()
        {
            StringBuilder json = new StringBuilder(512);
            json.Append('{');
            json.Append("\"schema\":\"uia-focus-event/1\",");
            json.Append("\"seq\":").Append(seq).Append(',');
            json.Append("\"label\":").Append(J(label)).Append(',');
            json.Append("\"kind\":").Append(J(kind)).Append(',');
            json.Append("\"time_utc\":").Append(J(utc.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"time_local\":").Append(J(local.ToString("yyyy-MM-ddTHH:mm:ss.fffzzz", CultureInfo.InvariantCulture))).Append(',');
            json.Append("\"since_start_ms\":").Append(since_start_ms).Append(',');
            json.Append("\"event_id\":").Append(event_id).Append(',');
            json.Append("\"has_element\":").Append(has_element ? "true" : "false").Append(',');
            json.Append("\"runtime_id\":").Append(J(runtime_id)).Append(',');
            json.Append("\"name\":").Append(J(name)).Append(',');
            json.Append("\"control_type\":").Append(J(control_type)).Append(',');
            json.Append("\"class_name\":").Append(J(class_name)).Append(',');
            json.Append("\"automation_id\":").Append(J(automation_id)).Append(',');
            json.Append("\"process_id\":").Append(process_id).Append(',');
            json.Append("\"is_target_process\":").Append(process_id == targetPid ? "true" : "false").Append(',');
            json.Append("\"keyboard_focusable\":").Append(Jb(keyboard_focusable)).Append(',');
            json.Append("\"enabled\":").Append(Jb(enabled)).Append(',');
            json.Append("\"has_keyboard_focus\":").Append(Jb(has_keyboard_focus)).Append(',');
            json.Append("\"bounds\":");
            if (bounds == null || bounds.Length != 4) { json.Append("null"); }
            else
            {
                json.Append('[');
                for (int index = 0; index < 4; index++)
                {
                    if (index > 0) { json.Append(','); }
                    json.Append(bounds[index].ToString("0.##", CultureInfo.InvariantCulture));
                }
                json.Append(']');
            }
            json.Append(',');
            json.Append("\"focused_runtime_id\":").Append(J(focused_runtime_id)).Append(',');
            json.Append("\"focused_name\":").Append(J(focused_name)).Append(',');
            json.Append("\"focused_process_id\":").Append(focused_process_id).Append(',');
            json.Append("\"note\":").Append(J(note));
            json.Append('}');
            return json.ToString();
        }
    }
}
