using System.Collections.Concurrent;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace OpenPixels.Desktop.Engine;

/// <summary>The result of an engine call: JSON (possibly null) and bytes (possibly empty).</summary>
public sealed record EngineReply(JsonNode? Json, byte[] Data);

public class EngineException(string message) : Exception(message);

/// <summary>A failure whose message was written, and translated, for the person using the app.</summary>
public sealed class UserMessageException(string message) : EngineException(message);

/// <summary>
/// The Rust engine, driven from one dedicated thread. The engine handles one call at a
/// time, so every call is queued here and the UI thread never waits on the GPU.
/// </summary>
public sealed class EngineClient : IDisposable
{
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };

    private readonly BlockingCollection<Action> _work = new();
    private readonly Thread _thread;
    private nint _engine;

    public EngineClient()
    {
        var started = new TaskCompletionSource();
        _thread = new Thread(() =>
        {
            try
            {
                _engine = NativeMethods.Create(out var error);
                if (_engine == 0)
                {
                    var message = Encoding.UTF8.GetString(error.ToArray());
                    NativeMethods.Free(error);
                    started.SetException(new EngineException(message));
                    return;
                }
                started.SetResult();
            }
            catch (Exception exception)
            {
                started.SetException(exception);
                return;
            }
            foreach (var work in _work.GetConsumingEnumerable()) work();
            NativeMethods.Destroy(_engine);
        })
        { IsBackground = true, Name = "OpenPixels engine" };
        _thread.Start();
        Ready = started.Task;
    }

    /// <summary>Completes once the GPU is set up, or fails with the reason.</summary>
    public Task Ready { get; }

    /// <summary>Calls an engine method with arguments serialised as camelCase JSON.</summary>
    public Task<EngineReply> CallAsync(string method, object? args = null, byte[]? data = null) =>
        CallRawAsync(method, args is null ? null : JsonSerializer.Serialize(args, JsonOptions), data);

    /// <summary>Calls an engine method with arguments that are already JSON.</summary>
    public Task<EngineReply> CallRawAsync(string method, string? json, byte[]? data = null)
    {
        var reply = new TaskCompletionSource<EngineReply>(TaskCreationOptions.RunContinuationsAsynchronously);
        _work.Add(() =>
        {
            try
            {
                reply.SetResult(Invoke(method, json, data));
            }
            catch (Exception exception)
            {
                reply.SetException(exception);
            }
        });
        return reply.Task;
    }

    private unsafe EngineReply Invoke(string method, string? json, byte[]? data)
    {
        var methodBytes = Encoding.UTF8.GetBytes(method);
        var argsBytes = json is null ? [] : Encoding.UTF8.GetBytes(json);
        data ??= [];
        OpBuffer jsonOut, dataOut;
        int status;
        fixed (byte* m = methodBytes)
        fixed (byte* a = argsBytes)
        fixed (byte* d = data)
        {
            status = NativeMethods.Call(_engine, m, (nuint)methodBytes.Length, a, (nuint)argsBytes.Length, d, (nuint)data.Length, out jsonOut, out dataOut);
        }
        try
        {
            var text = Encoding.UTF8.GetString(jsonOut.ToArray());
            if (status != 0) throw new EngineException(text.Length > 0 ? text : $"{method} failed");
            return new EngineReply(text.Length > 0 ? JsonNode.Parse(text) : null, dataOut.ToArray());
        }
        finally
        {
            NativeMethods.Free(jsonOut);
            NativeMethods.Free(dataOut);
        }
    }

    public void Dispose()
    {
        _work.CompleteAdding();
        _thread.Join(TimeSpan.FromSeconds(5));
    }
}
