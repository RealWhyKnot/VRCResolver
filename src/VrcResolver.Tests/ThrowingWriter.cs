using System.Text;

namespace VrcResolver.Tests;

internal sealed class ThrowingWriter : TextWriter
{
    public override Encoding Encoding => Encoding.UTF8;

    public override void Write(char value) => throw Failure();
    public override void Write(string? value) => throw Failure();
    public override void WriteLine() => throw Failure();
    public override void WriteLine(string? value) => throw Failure();
    public override void Flush() => throw Failure();

    private static IOException Failure() => new("The handle is invalid.");
}
