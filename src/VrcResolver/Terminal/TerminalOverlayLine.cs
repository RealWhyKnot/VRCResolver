namespace VrcResolver;

internal sealed class TerminalOverlayLine
{
    private int _renderedLength;
    private string _lastRendered = "";
    private bool _visible;

    public int RenderedLength => _renderedLength;
    public string LastRendered => _lastRendered;
    public bool IsVisible => _visible;

    public void Clear(TextWriter writer)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        if (!_visible || _renderedLength <= 0) return;

        writer.Write('\r');
        writer.Write(new string(' ', _renderedLength));
        writer.Write('\r');
        _renderedLength = 0;
        _visible = false;
    }

    public void Render(TextWriter writer, string line)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        line ??= "";

        Clear(writer);
        if (line.Length == 0) return;

        writer.Write(line);
        _renderedLength = line.Length;
        _lastRendered = line;
        _visible = true;
    }

    public void Render(
        TextWriter writer,
        TerminalFrame frame,
        Action<TextWriter, IReadOnlyList<TerminalTextRun>> writeFrame)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        if (frame == null) throw new ArgumentNullException(nameof(frame));
        if (writeFrame == null) throw new ArgumentNullException(nameof(writeFrame));

        Clear(writer);
        if (frame.PlainText.Length == 0) return;

        writeFrame(writer, frame.Runs);
        _renderedLength = frame.PlainText.Length;
        _lastRendered = frame.PlainText;
        _visible = true;
    }

    public bool RenderIfChanged(TextWriter writer, string line)
    {
        return RenderIfChanged(writer, line, static (w, text) => w.Write(text));
    }

    public bool RenderIfChanged(TextWriter writer, string line, Action<TextWriter, string> writeLine)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        if (writeLine == null) throw new ArgumentNullException(nameof(writeLine));
        line ??= "";

        if (_visible && string.Equals(_lastRendered, line, StringComparison.Ordinal))
            return false;

        Clear(writer);
        if (line.Length == 0)
        {
            _lastRendered = line;
            return false;
        }

        writeLine(writer, line);
        _renderedLength = line.Length;
        _lastRendered = line;
        _visible = true;
        return line.Length > 0;
    }

    public bool RenderIfChanged(
        TextWriter writer,
        TerminalFrame frame,
        Action<TextWriter, IReadOnlyList<TerminalTextRun>> writeFrame)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        if (frame == null) throw new ArgumentNullException(nameof(frame));
        if (writeFrame == null) throw new ArgumentNullException(nameof(writeFrame));

        if (_visible && string.Equals(_lastRendered, frame.PlainText, StringComparison.Ordinal))
            return false;

        Clear(writer);
        if (frame.PlainText.Length == 0)
        {
            _lastRendered = frame.PlainText;
            return false;
        }

        writeFrame(writer, frame.Runs);
        _renderedLength = frame.PlainText.Length;
        _lastRendered = frame.PlainText;
        _visible = true;
        return true;
    }
}
