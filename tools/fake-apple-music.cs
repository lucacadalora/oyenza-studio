// A stand-in for Apple Music for Windows in CI: a window in a process named
// AppleMusic.exe with the two controls Oyen Poni presses through UI
// Automation, under Apple Music's automation ids (a progress slider
// "LCDScrubber" counting seconds and a back button
// "TransportControl_SkipBack"). Every change lands in the log file named on
// the command line, one line each: "slider <seconds>" or "back".
//
// Built by CI with .NET Framework's own compiler (C# 5, WPF), see ci.yml:
//   csc /target:winexe /out:AppleMusic.exe /r:<WPF>\PresentationFramework.dll
//       /r:<WPF>\PresentationCore.dll /r:<WPF>\WindowsBase.dll /r:System.Xaml.dll
//       tools\fake-apple-music.cs

using System;
using System.Globalization;
using System.IO;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

static class FakeAppleMusic
{
    static string log;

    static void Write(string line)
    {
        File.AppendAllText(log, line + Environment.NewLine);
    }

    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length != 1)
        {
            return 2;
        }
        log = args[0];
        File.WriteAllText(log, "");

        var slider = new Slider { Minimum = 0, Maximum = 240, Value = 30, Width = 300 };
        AutomationProperties.SetAutomationId(slider, "LCDScrubber");
        slider.ValueChanged += (s, e) => Write("slider " + e.NewValue.ToString("0.###", CultureInfo.InvariantCulture));

        var back = new Button { Content = "Back", Width = 80 };
        AutomationProperties.SetAutomationId(back, "TransportControl_SkipBack");
        back.Click += (s, e) => Write("back");

        var lcd = new StackPanel();
        AutomationProperties.SetAutomationId(lcd, "LCD");
        lcd.Children.Add(slider);
        var bar = new StackPanel { Orientation = Orientation.Horizontal };
        AutomationProperties.SetAutomationId(bar, "TransportBar");
        bar.Children.Add(back);
        bar.Children.Add(lcd);

        var window = new Window { Title = "Apple Music", Width = 480, Height = 120, Content = bar };
        window.Loaded += (s, e) => Write("ready");
        return new Application().Run(window);
    }
}
