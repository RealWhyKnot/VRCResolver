using System.Collections.Concurrent;
using System.Net;
using System.Net.Http.Headers;
using System.Text;

namespace VrcResolver.Tests;

internal sealed class ScriptedUpstream : HttpMessageHandler
{
    public const int BodyLength = 256 * 1024;
    public const int SegmentCount = 3;
    private int _openBodies;

    public int OpenBodies => Volatile.Read(ref _openBodies);
    public ConcurrentQueue<string> Requests { get; } = new();
    public ConcurrentQueue<string> RangeHeaders { get; } = new();

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        Uri uri = request.RequestUri!;
        Requests.Enqueue(uri.ToString());
        if (request.Headers.Range != null) RangeHeaders.Enqueue(request.Headers.Range.ToString());
        string mode = System.Web.HttpUtility.ParseQueryString(uri.Query)["mode"] ?? "full";

        switch (mode)
        {
            case "stall-headers":
                await Task.Delay(Timeout.Infinite, cancellationToken);
                throw new InvalidOperationException();
            case "status-500":
                return Text(HttpStatusCode.InternalServerError, "error", "text/plain");
            case "status-404":
                return Text(HttpStatusCode.NotFound, "gone", "text/plain");
            case "manifest":
                return Text(HttpStatusCode.OK, Manifest(), "application/vnd.apple.mpegurl");
            case "manifest-stall":
                return Body(new ScriptedStream(this, Encoding.ASCII.GetBytes("#EXTM3U\n"), Tail.Stall), null, "application/vnd.apple.mpegurl");
            case "endless":
                return Body(new ScriptedStream(this, Array.Empty<byte>(), Tail.Endless), null, "audio/mpeg");
            case "short":
                return Body(new ScriptedStream(this, new byte[1000], Tail.End), BodyLength, "video/mp2t");
            case "reset":
                return Body(new ScriptedStream(this, new byte[1000], Tail.Throw), BodyLength, "video/mp2t");
            case "stall-body":
                return Body(new ScriptedStream(this, new byte[1000], Tail.Stall), BodyLength, "video/mp2t");
            default:
                return Full(request);
        }
    }

    private HttpResponseMessage Full(HttpRequestMessage request)
    {
        byte[] body = new byte[BodyLength];
        for (int i = 0; i < body.Length; i++) body[i] = (byte)i;
        long from = request.Headers.Range?.Ranges.FirstOrDefault()?.From ?? -1;
        if (from < 0)
            return Body(new ScriptedStream(this, body, Tail.End), BodyLength, "video/mp2t");

        byte[] slice = body[(int)from..];
        var resp = Body(new ScriptedStream(this, slice, Tail.End), slice.Length, "video/mp2t");
        resp.StatusCode = HttpStatusCode.PartialContent;
        resp.Content.Headers.ContentRange = new ContentRangeHeaderValue(from, BodyLength - 1, BodyLength);
        return resp;
    }

    private static string Manifest()
    {
        var sb = new StringBuilder("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n");
        for (int i = 0; i < SegmentCount; i++)
        {
            sb.Append("#EXTINF:4.0,\n");
            sb.Append(i % 2 == 0
                ? "https://vrcresolver.com/api/proxy/seg" + i + ".ts?mode=full&i=" + i + "\n"
                : "seg" + i + ".ts?mode=full&i=" + i + "\n");
        }
        sb.Append("#EXT-X-ENDLIST\n");
        return sb.ToString();
    }

    private static HttpResponseMessage Text(HttpStatusCode status, string body, string contentType)
    {
        var content = new StringContent(body, Encoding.UTF8);
        content.Headers.ContentType = new MediaTypeHeaderValue(contentType);
        return new HttpResponseMessage(status) { Content = content };
    }

    private static HttpResponseMessage Body(Stream body, long? length, string contentType)
    {
        var content = new StreamContent(body);
        content.Headers.ContentLength = length;
        content.Headers.ContentType = new MediaTypeHeaderValue(contentType);
        return new HttpResponseMessage(HttpStatusCode.OK) { Content = content };
    }

    internal void BodyOpened() => Interlocked.Increment(ref _openBodies);
    internal void BodyClosed() => Interlocked.Decrement(ref _openBodies);
}

internal enum Tail { End, Throw, Stall, Endless }

internal sealed class ScriptedStream : Stream
{
    private readonly ScriptedUpstream _owner;
    private readonly byte[] _prefix;
    private readonly Tail _tail;
    private int _offset;
    private int _disposed;

    public ScriptedStream(ScriptedUpstream owner, byte[] prefix, Tail tail)
    {
        _owner = owner;
        _prefix = prefix;
        _tail = tail;
        owner.BodyOpened();
    }

    public override bool CanRead => true;
    public override bool CanSeek => false;
    public override bool CanWrite => false;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => _offset; set => throw new NotSupportedException(); }
    public override void Flush() { }
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();
    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

    public override int Read(byte[] buffer, int offset, int count)
        => ReadAsync(buffer.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();

    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        if (_offset < _prefix.Length)
        {
            int n = Math.Min(buffer.Length, _prefix.Length - _offset);
            _prefix.AsMemory(_offset, n).CopyTo(buffer);
            _offset += n;
            return n;
        }

        switch (_tail)
        {
            case Tail.Throw:
                throw new IOException("upstream reset");
            case Tail.Stall:
                await Task.Delay(Timeout.Infinite, cancellationToken);
                return 0;
            case Tail.Endless:
                await Task.Delay(1, cancellationToken);
                int n = Math.Min(buffer.Length, 16 * 1024);
                buffer.Span[..n].Fill(0x55);
                return n;
            default:
                return 0;
        }
    }

    protected override void Dispose(bool disposing)
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 0) _owner.BodyClosed();
        base.Dispose(disposing);
    }
}
