using System;
using System.Reflection;
using System.Threading;
using System.Threading.Tasks;
using MessagePack;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class V31BinaryControlFrameTests
{
    private static byte[] EncodeSingleStringFrame(string action)
    {
        var buffer = new System.Buffers.ArrayBufferWriter<byte>();
        var writer = new MessagePackWriter(buffer);
        writer.WriteArrayHeader(1);
        writer.Write(action);
        writer.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    private static async Task DispatchBinaryAsync(VrcResolver.MeshClient client, byte[] payload)
    {
        var method = typeof(VrcResolver.MeshClient).GetMethod("DispatchBinaryFrameAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        var task = (Task)method!.Invoke(client, new object[] { payload, CancellationToken.None })!;
        await task;
    }

    private static DateTime ReadLastPongUtc(VrcResolver.MeshClient client)
    {
        var field = typeof(VrcResolver.MeshClient).GetField("_lastPongUtc",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (DateTime)field!.GetValue(client)!;
    }

    [Fact]
    public async Task Binary_pong_advances_lastPongUtc()
    {
        var client = new VrcResolver.MeshClient();
        DateTime before = ReadLastPongUtc(client);

        byte[] payload = EncodeSingleStringFrame(WireConstants.ActionPong);
        await DispatchBinaryAsync(client, payload);

        DateTime after = ReadLastPongUtc(client);
        Assert.True(after > before,
            "expected _lastPongUtc to advance after binary pong dispatch; got " + after);
    }

    [Fact]
    public async Task Binary_ping_does_not_throw()
    {
        var client = new VrcResolver.MeshClient();
        byte[] payload = EncodeSingleStringFrame(WireConstants.ActionPing);
        await DispatchBinaryAsync(client, payload);
    }

    [Fact]
    public async Task Binary_unknown_action_still_falls_through_to_default()
    {
        byte[] payload = EncodeSingleStringFrame("frobnicate_widget");

        var client = new VrcResolver.MeshClient();
        await DispatchBinaryAsync(client, payload);
    }
}
