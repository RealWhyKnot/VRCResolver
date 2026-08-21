using System.Text;

namespace VrcResolver;

internal sealed class TerminalInputBuffer
{
    private const int MaxInputChars = 512;
    private const int MaxHistory = 200;
    private readonly object _lock = new();
    private readonly StringBuilder _buffer = new();
    private readonly List<string> _history;
    private int _historyCursor;

    public TerminalInputBuffer(IEnumerable<string>? history = null)
    {
        _history = history?
            .Where(s => !string.IsNullOrWhiteSpace(s))
            .Select(s => s.Trim())
            .TakeLast(MaxHistory)
            .ToList()
            ?? new List<string>();
        _historyCursor = _history.Count;
    }

    public string Text()
    {
        lock (_lock) return _buffer.ToString();
    }

    public void Append(char ch)
    {
        lock (_lock)
        {
            if (_buffer.Length < MaxInputChars)
                _buffer.Append(ch);
            _historyCursor = _history.Count;
        }
    }

    public void Backspace()
    {
        lock (_lock)
        {
            if (_buffer.Length > 0)
                _buffer.Length--;
        }
    }

    public void Clear()
    {
        lock (_lock)
        {
            _buffer.Clear();
            _historyCursor = _history.Count;
        }
    }

    public string Take()
    {
        lock (_lock)
        {
            string value = _buffer.ToString();
            _buffer.Clear();
            _historyCursor = _history.Count;
            return value;
        }
    }

    public void Set(string value)
    {
        lock (_lock)
        {
            _buffer.Clear();
            if (!string.IsNullOrEmpty(value))
                _buffer.Append(value.Length <= MaxInputChars ? value : value[^MaxInputChars..]);
            _historyCursor = _history.Count;
        }
    }

    public void Remember(string command)
    {
        command = (command ?? "").Trim();
        if (command.Length == 0) return;

        lock (_lock)
        {
            if (_history.Count == 0 || !string.Equals(_history[^1], command, StringComparison.Ordinal))
            {
                _history.Add(command);
                if (_history.Count > MaxHistory)
                    _history.RemoveRange(0, _history.Count - MaxHistory);
            }
            _historyCursor = _history.Count;
        }
    }

    public void PreviousHistory()
    {
        lock (_lock)
        {
            if (_history.Count == 0) return;
            _historyCursor = Math.Max(0, _historyCursor - 1);
            ReplaceWithHistoryLocked();
        }
    }

    public void NextHistory()
    {
        lock (_lock)
        {
            if (_history.Count == 0) return;
            _historyCursor = Math.Min(_history.Count, _historyCursor + 1);
            if (_historyCursor == _history.Count)
                _buffer.Clear();
            else
                ReplaceWithHistoryLocked();
        }
    }

    private void ReplaceWithHistoryLocked()
    {
        _buffer.Clear();
        _buffer.Append(_history[_historyCursor]);
    }
}
