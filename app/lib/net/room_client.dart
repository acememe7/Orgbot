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

/// What a room connection offers, whichever transport it uses.
///
/// The socket client and the in-process client both satisfy this, so the store
/// and the widgets above cannot tell which one they were handed — which is how
/// a phone runs the same room code as the desktop.
abstract class RoomLink {
  /// Wire messages arriving from the room.
  Stream<Map<String, Object?>> get messages;

  /// Connection state changes.
  Stream<LinkState> get connectionState;

  /// The current connection state.
  LinkState get current;

  /// Send a raw wire message.
  void send(Map<String, Object?> message);

  /// Say something to the room as a person.
  void sendMessage(String author, String body, {String? taskId});

  /// Answer a bot that asked a question.
  void answerBot(String task, String answer);

  /// Pause the room.
  void pause({String reason});

  /// Resume the room.
  void resume();

  /// Ask for a fresh snapshot.
  void resync();

  /// Connect. An in-process link is already connected.
  Future<void> connect(String url);

  /// Disconnect and release resources.
  Future<void> close();
}

/// A live connection to one room.
class RoomClient implements RoomLink {
  RoomClient();

  WebSocketChannel? _channel;
  StreamSubscription<Object?>? _subscription;

  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();

  /// Decoded server messages, one per frame.
  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;

  /// Connection state transitions.
  @override
  Stream<LinkState> get connectionState => _state.stream;

  LinkState _current = LinkState.disconnected;

  /// The most recent connection state.
  @override
  LinkState get current => _current;

  void _setState(LinkState state) {
    _current = state;
    if (!_state.isClosed) _state.add(state);
  }

  /// Connect to a server. [url] is like `ws://127.0.0.1:8787/ws`.
  ///
  /// Any existing connection is torn down first, so calling this to reconnect
  /// is safe.
  @override
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
  @override
  void send(Map<String, Object?> message) {
    final channel = _channel;
    if (channel == null) return;
    channel.sink.add(jsonEncode(message));
  }

  // ---- typed client actions ------------------------------------------------

  /// Post a human message into the room.
  @override
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
  @override
  void answerBot(String taskId, String answer) {
    send({'type': 'answer_bot', 'task': taskId, 'answer': answer});
  }

  /// Pause the room.
  @override
  void pause({String reason = 'paused from the app'}) {
    send({'type': 'pause', 'reason': reason});
  }

  /// Resume a paused room.
  @override
  void resume() => send({'type': 'resume'});

  /// Request a fresh snapshot.
  @override
  void resync() => send({'type': 'resync'});

  Future<void> _teardown() async {
    await _subscription?.cancel();
    _subscription = null;
    await _channel?.sink.close();
    _channel = null;
  }

  /// Close the connection and release resources.
  @override
  Future<void> close() async {
    await _teardown();
    _setState(LinkState.disconnected);
    await _messages.close();
    await _state.close();
  }
}

/// A client for a room running inside this process.
///
/// The engine library hands back the same wire messages the WebSocket would
/// carry, so everything above this — the store, the widgets — cannot tell which
/// transport it got. That is the point: one room implementation, two ways in.
class LocalRoomClient implements RoomLink {
  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();

  /// Messages this client was asked to send, for tests.
  final List<Map<String, Object?>> sent = [];

  /// Feed a message in, as if it had arrived over the socket.
  void deliver(Map<String, Object?> message) {
    if (_messages.isClosed) return;
    _messages.add(message);
  }

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;

  @override
  Stream<LinkState> get connectionState => _state.stream;

  @override
  LinkState get current => LinkState.connected;

  @override
  void send(Map<String, Object?> message) => sent.add(message);

  @override
  void sendMessage(String author, String body, {String? taskId}) =>
      sent.add({'type': 'send_message', 'author': author, 'body': body});

  @override
  void answerBot(String task, String answer) =>
      sent.add({'type': 'answer', 'task': task, 'answer': answer});

  @override
  void pause({String reason = ''}) =>
      sent.add({'type': 'pause', 'reason': reason});

  @override
  void resume() => sent.add({'type': 'resume'});

  @override
  void resync() => sent.add({'type': 'resync'});

  /// Nothing to connect to; the engine is already running here.
  @override
  Future<void> connect(String url) async {}

  @override
  Future<void> close() async {
    await _messages.close();
    await _state.close();
  }
}
