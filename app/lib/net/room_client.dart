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
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  });

  /// Answer a bot that asked a question.
  void answerBot(String task, String answer);

  /// Answer a question the room put to you.
  ///
  /// Addressed to the decision, not to a task: one answer releases every task
  /// that was waiting on that same question.
  void answerDecision(String decision, String chosen, {String note = ''});

  /// Pause the room.
  void pause({String reason});

  /// Resume the room.
  void resume();

  /// Ask for a fresh snapshot.
  void resync();

  /// Connect. An in-process link is already connected.
  ///
  /// [token] is the room's key, sent in the opening frame. A room reachable
  /// from another machine always has one; a loopback room may not.
  Future<void> connect(String url, {String? token});

  /// Disconnect and release resources.
  Future<void> close();
}

/// A live connection to one room.
class RoomClient implements RoomLink {
  RoomClient();
  bool _closed = false;
  Completer<void>? _admitted;

  WebSocketChannel? _channel;
  StreamSubscription<Object?>? _subscription;

  /// What the person said while the link was down, delivered in order as
  /// soon as it is up again. Without this a message sent during a reconnect
  /// stayed on screen as pending and never reached the room.
  final List<Map<String, Object?>> _outbox = [];

  static const _personInput = {'send_message', 'answer_decision', 'answer_bot'};

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

  /// Connect to a server. [url] is like `wss://box.example.com/ws`.
  ///
  /// Any existing connection is torn down first, so calling this to reconnect
  /// is safe. The token travels in the opening frame rather than the URL,
  /// because a URL ends up in logs, history and crash reports.
  @override
  Future<void> connect(String url, {String? token}) async {
    if (_closed) return;
    await _teardown();
    _setState(LinkState.connecting);
    try {
      final uri = Uri.parse(url);
      if (uri.scheme != 'wss' &&
          !(uri.scheme == 'ws' &&
              ['127.0.0.1', '::1', 'localhost'].contains(uri.host))) {
        throw const FormatException('Remote room connections require wss://.');
      }
      final channel = WebSocketChannel.connect(uri);
      _channel = channel;
      // `ready` throws if the handshake fails, which lets us report a clean
      // error state instead of a silent dead socket.
      await channel.ready.timeout(const Duration(seconds: 10));
      if (_closed) {
        await channel.sink.close();
        return;
      }
      final admitted = Completer<void>();
      _admitted = admitted;
      // Attach an error handler before listening; refusal can arrive immediately.
      final admission = admitted.future.timeout(const Duration(seconds: 15));
      send({
        'type': 'hello',
        'wire_version': wireVersion,
        if (token != null && token.trim().isNotEmpty) 'token': token.trim(),
      });

      _subscription = channel.stream.listen(
        _onFrame,
        onError: (Object error) {
          if (!admitted.isCompleted) {
            admitted.completeError(StateError('Room connection failed.'));
          }
          _setState(LinkState.error);
        },
        onDone: () {
          if (!admitted.isCompleted) {
            admitted.completeError(
              StateError('Room closed before authentication.'),
            );
          }
          _setState(LinkState.closed);
        },
        cancelOnError: false,
      );
      await admission;
      if (!_closed) {
        _setState(LinkState.connected);
        _flushOutbox();
      }
    } catch (error) {
      if (!_messages.isClosed) {
        _messages.add({'type': 'error', 'message': error.toString()});
      }
      _setState(LinkState.error);
      await _teardown();
    }
  }

  void _onFrame(Object? frame) {
    if (frame is! String) return;
    try {
      final decoded = jsonDecode(frame);
      if (decoded is Map) {
        final admitted = _admitted;
        if (decoded['type'] == 'snapshot' &&
            admitted != null &&
            !admitted.isCompleted) {
          if (decoded['wire_version'] == wireVersion) {
            admitted.complete();
          } else {
            admitted.completeError(StateError('Incompatible room protocol.'));
          }
        } else if (decoded['type'] == 'closed' &&
            admitted != null &&
            !admitted.isCompleted) {
          admitted.completeError(
            StateError(decoded['reason'] as String? ?? 'Room access refused.'),
          );
        }
        if (!_messages.isClosed) _messages.add(decoded.cast<String, Object?>());
      }
    } catch (_) {
      // A malformed frame is dropped rather than killing the stream.
    }
  }

  /// Send a client message as JSON.
  @override
  void send(Map<String, Object?> message) {
    // The person's own input waits for the link rather than vanishing.
    if (_personInput.contains(message['type']) &&
        (_current != LinkState.connected || _channel == null)) {
      _outbox.add(message);
      return;
    }
    final channel = _channel;
    if (channel == null) return;
    try {
      channel.sink.add(jsonEncode(message));
    } on StateError {
      // The socket closed under us; keep the input for the reconnect.
      if (_personInput.contains(message['type'])) _outbox.add(message);
    }
  }

  void _flushOutbox() {
    final waiting = List.of(_outbox);
    _outbox.clear();
    waiting.forEach(send);
  }

  // ---- typed client actions ------------------------------------------------

  /// Post a human message into the room.
  @override
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  }) {
    final message = <String, Object?>{
      'type': 'send_message',
      'author': author,
      'body': body,
    };
    if (taskId != null) message['task'] = taskId;
    if (replyTo != null) message['reply_to'] = replyTo;
    send(message);
  }

  /// Answer a bot blocked awaiting a human.
  @override
  void answerBot(String taskId, String answer) {
    send({'type': 'answer_bot', 'task': taskId, 'answer': answer});
  }

  @override
  void answerDecision(String decision, String chosen, {String note = ''}) {
    send({
      'type': 'answer_decision',
      'decision': decision,
      'chosen': chosen,
      'note': note,
    });
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
    _closed = true;
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
  final void Function(Map<String, Object?>)? onSend;
  LocalRoomClient({this.onSend});
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
  void send(Map<String, Object?> message) {
    if (onSend != null) {
      onSend!(message);
    } else {
      sent.add(message);
    }
  }

  @override
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  }) => send({
    'type': 'send_message',
    'author': author,
    'body': body,
    'task': ?taskId,
    'reply_to': ?replyTo,
  });

  @override
  void answerBot(String task, String answer) =>
      send({'type': 'answer_bot', 'task': task, 'answer': answer});

  @override
  void answerDecision(String decision, String chosen, {String note = ''}) =>
      send({
        'type': 'answer_decision',
        'decision': decision,
        'chosen': chosen,
        'note': note,
      });

  @override
  void pause({String reason = ''}) => send({'type': 'pause', 'reason': reason});

  @override
  void resume() => send({'type': 'resume'});

  @override
  void resync() => send({'type': 'resync'});

  /// Nothing to connect to; the engine is already running here, so there is
  /// no network for a token to protect.
  @override
  Future<void> connect(String url, {String? token}) async {}

  @override
  Future<void> close() async {
    await _messages.close();
    await _state.close();
  }
}
