/// A thin WebSocket connection to a orgbot server.
///
/// This owns the socket and nothing else: it decodes inbound frames to JSON
/// maps and forwards them, and it encodes outbound client messages. All
/// interpretation of the protocol lives in [RoomStore], so the transport can be
/// swapped (for tests, or an embedded FFI engine later) without touching the
/// state logic.
library;

import 'dart:async';
import 'dart:convert';

import 'package:web_socket_channel/web_socket_channel.dart';

/// The wire version this client speaks. Must match the server's `WIRE_VERSION`.
const int wireVersion = 1;

/// Connection lifecycle, surfaced to the UI.
enum LinkState { disconnected, connecting, connected, closed, error }

/// A live connection to one room.
class RoomClient {
  RoomClient();

  WebSocketChannel? _channel;
  StreamSubscription<Object?>? _subscription;

  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();

  /// Decoded server messages, one per frame.
  Stream<Map<String, Object?>> get messages => _messages.stream;

  /// Connection state transitions.
  Stream<LinkState> get connectionState => _state.stream;

  LinkState _current = LinkState.disconnected;

  /// The most recent connection state.
  LinkState get current => _current;

  void _setState(LinkState state) {
    _current = state;
    if (!_state.isClosed) _state.add(state);
  }

  /// Connect to a server. [url] is like `ws://127.0.0.1:8787/ws`.
  ///
  /// Any existing connection is torn down first, so calling this to reconnect
  /// is safe.
  Future<void> connect(String url) async {
    await _teardown();
    _setState(LinkState.connecting);
    try {
      final channel = WebSocketChannel.connect(Uri.parse(url));
      _channel = channel;
      // `ready` throws if the handshake fails, which lets us report a clean
      // error state instead of a silent dead socket.
      await channel.ready;
      _setState(LinkState.connected);
      send({'type': 'hello', 'wire_version': wireVersion});

      _subscription = channel.stream.listen(
        _onFrame,
        onError: (Object error) => _setState(LinkState.error),
        onDone: () => _setState(LinkState.closed),
        cancelOnError: false,
      );
    } catch (_) {
      _setState(LinkState.error);
    }
  }

  void _onFrame(Object? frame) {
    if (frame is! String) return;
    try {
      final decoded = jsonDecode(frame);
      if (decoded is Map) {
        _messages.add(decoded.cast<String, Object?>());
      }
    } catch (_) {
      // A malformed frame is dropped rather than killing the stream.
    }
  }

  /// Send a client message as JSON.
  void send(Map<String, Object?> message) {
    final channel = _channel;
    if (channel == null) return;
    channel.sink.add(jsonEncode(message));
  }

  // ---- typed client actions ------------------------------------------------

  /// Post a human message into the room.
  void sendMessage(String author, String body, {String? taskId}) {
    final message = <String, Object?>{
      'type': 'send_message',
      'author': author,
      'body': body,
    };
    if (taskId != null) message['task'] = taskId;
    send(message);
  }

  /// Answer a bot blocked awaiting a human.
  void answerBot(String taskId, String answer) {
    send({'type': 'answer_bot', 'task': taskId, 'answer': answer});
  }

  /// Pause the room.
  void pause({String reason = 'paused from the app'}) {
    send({'type': 'pause', 'reason': reason});
  }

  /// Resume a paused room.
  void resume() => send({'type': 'resume'});

  /// Request a fresh snapshot.
  void resync() => send({'type': 'resync'});

  Future<void> _teardown() async {
    await _subscription?.cancel();
    _subscription = null;
    await _channel?.sink.close();
    _channel = null;
  }

  /// Close the connection and release resources.
  Future<void> close() async {
    await _teardown();
    _setState(LinkState.disconnected);
    await _messages.close();
    await _state.close();
  }
}
