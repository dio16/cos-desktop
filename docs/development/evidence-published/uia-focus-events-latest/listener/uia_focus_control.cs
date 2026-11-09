// Test-only UIA focus-event control target for the native AX audit.
//
// A plain WinForms window whose focus moves between three named controls on a
// timer. It exists to prove that the listener's event subscription and its
// FocusedElement polling observe real focus transitions on this host, so a
// negative result against the product window can be attributed to the product
// provider rather than to the measurement path. It is never part of the product.
//
// Build (Windows PowerShell 5.1 / .NET Framework 4.x csc):
//   csc.exe -nologo -out:uia_focus_control.exe uia_focus_control.cs
//         -r:System.Windows.Forms.dll -r:System.Drawing.dll
//
// Usage: uia_focus_control.exe [seconds]
// Exit code 0 after the form closes.

using System;
using System.Drawing;
using System.Windows.Forms;

public static class UiaFocusControl
{
    [STAThread]
    public static int Main(string[] args)
    {
        int seconds = 20;
        if (args.Length > 0) { int.TryParse(args[0], out seconds); }
        if (seconds < 5) { seconds = 5; }

        Application.EnableVisualStyles();
        Form form = new Form();
        form.Text = "uia-focus-control";
        form.ClientSize = new Size(420, 220);
        form.StartPosition = FormStartPosition.CenterScreen;

        TextBox first = new TextBox();
        first.Text = "control-a";
        first.SetBounds(20, 20, 240, 28);
        first.Name = "control-a";

        TextBox second = new TextBox();
        second.Text = "control-b";
        second.SetBounds(20, 64, 240, 28);
        second.Name = "control-b";

        Button third = new Button();
        third.Text = "control-c";
        third.SetBounds(20, 108, 240, 40);
        third.Name = "control-c";

        form.Controls.Add(first);
        form.Controls.Add(second);
        form.Controls.Add(third);

        Timer timer = new Timer();
        timer.Interval = 700;
        int tick = 0;
        int ticks = (seconds * 1000) / 700;
        timer.Tick += delegate(object sender, EventArgs eventArgs)
        {
            tick++;
            if (tick % 3 == 1) { second.Focus(); }
            else if (tick % 3 == 2) { third.Focus(); }
            else { first.Focus(); }
            if (tick >= ticks) { timer.Stop(); form.Close(); }
        };
        form.Shown += delegate(object sender, EventArgs eventArgs)
        {
            first.Focus();
            timer.Start();
        };
        Application.Run(form);
        return 0;
    }
}
