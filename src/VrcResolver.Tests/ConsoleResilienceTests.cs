using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ConsoleResilienceTests : IDisposable
{
    private readonly TextWriter _originalOut = Console.Out;
    private readonly TextWriter _originalErr = Console.Error;
    private readonly string _root;

    public ConsoleResilienceTests()
    {
        _root = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-console-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_root);
    }

    public void Dispose()
    {
        Logger.Close();
        Console.SetOut(_originalOut);
        Console.SetError(_originalErr);
        try { Directory.Delete(_root, recursive: true); } catch { }
    }

    [Fact]
    public void ConsoleUx_survives_a_throwing_console()
    {
        Console.SetOut(new ThrowingWriter());

        Exception? thrown = Record.Exception(() =>
        {
            ConsoleUx.Write(LogComponent.Patch, "hello");
            ConsoleUx.Success(LogComponent.Patch, "hello");
            ConsoleUx.Warn(LogComponent.Hosts, "hello");
            ConsoleUx.Error(LogComponent.Mesh, "hello");
            ConsoleUx.Fatal("hello");
        });

        Assert.Null(thrown);
    }

    [Fact]
    public void ConsoleUx_keeps_the_overlay_balanced_when_the_console_throws()
    {
        var overlay = new CountingOverlay();
        Console.SetOut(new ThrowingWriter());

        using IDisposable registration = ConsoleUx.UseOverlay(overlay);
        int clearsBefore = overlay.ClearCount;
        int rendersBefore = overlay.RenderCount;

        ConsoleUx.Write(LogComponent.Terminal, "hello");

        Assert.Equal(clearsBefore + 1, overlay.ClearCount);
        Assert.Equal(rendersBefore + 1, overlay.RenderCount);
    }

    [Fact]
    public void TeeWriter_writes_the_log_file_even_when_the_console_throws()
    {
        string logPath = Path.Combine(_root, "tee.log");
        Logger.InstallForTests(logPath, new ThrowingWriter());

        Exception? thrown = Record.Exception(() =>
        {
            Console.Out.WriteLine("line one");
            Console.Error.WriteLine("line two");
            Console.Out.WriteLine();
            Console.Out.WriteLine("line {0}", "three");
        });
        Logger.Close();

        Assert.Null(thrown);
        string text = File.ReadAllText(logPath);
        Assert.Contains("line one", text);
        Assert.Contains("line two", text);
        Assert.Contains("line three", text);
    }

    [Fact]
    public void TeeWriter_swallows_a_dead_console_for_partial_writes_and_flush()
    {
        Logger.InstallForTests(Path.Combine(_root, "partial.log"), new ThrowingWriter());

        Exception? thrown = Record.Exception(() =>
        {
            Console.Out.Write("partial");
            Console.Out.Write('x');
            Console.Out.Flush();
            Console.Error.Flush();
        });

        Assert.Null(thrown);
    }

    [Fact]
    public void TeeWriter_still_reaches_a_working_console()
    {
        var console = new StringWriter();
        string logPath = Path.Combine(_root, "both.log");
        Logger.InstallForTests(logPath, console);

        Console.Out.WriteLine("both places");
        Logger.Close();

        Assert.Contains("both places", console.ToString());
        Assert.Contains("both places", File.ReadAllText(logPath));
    }

    private sealed class CountingOverlay : IConsoleOverlay
    {
        public int ClearCount { get; private set; }
        public int RenderCount { get; private set; }

        public void ClearLocked() => ClearCount++;

        public void RenderLocked() => RenderCount++;
    }
}
