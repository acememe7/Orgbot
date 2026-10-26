import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:orgbot_app/main.dart' show orgBotTheme;
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/widgets/objectives_pane.dart';
import 'package:orgbot_app/ui/workspace_changes_dialog.dart';

class _FixtureLink implements RoomLink {
  final _messages = StreamController<Map<String, Object?>>.broadcast(
    sync: true,
  );
  final _states = StreamController<LinkState>.broadcast(sync: true);

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _states.stream;
  @override
  LinkState get current => LinkState.connected;
  @override
  void send(Map<String, Object?> message) {}
  @override
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  }) {}
  @override
  void answerBot(String task, String answer) {}
  @override
  void answerDecision(String decision, String chosen, {String note = ''}) {}
  @override
  void pause({String reason = ''}) {}
  @override
  void resume() {}
  @override
  void resync() {}
  @override
  Future<void> connect(String url, {String? token}) async {}
  @override
  Future<void> close() async {
    await _messages.close();
    await _states.close();
  }
}

final _captureBoundary = GlobalKey();
final _navigator = GlobalKey<NavigatorState>();
final _fixtureState = GlobalKey<_UiAcceptanceState>();

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final captureDir = Directory(
    Platform.environment['ORGBOTS_UI_CAPTURE_DIR'] ??
        '/tmp/orgbots2-ui-capture',
  );
  await captureDir.create(recursive: true);
  final errors = <String>[];
  FlutterError.onError = (details) {
    errors.add(details.exceptionAsString());
  };
  ui.PlatformDispatcher.instance.onError = (error, stack) {
    errors.add('$error');
    return true;
  };
  runApp(
    MaterialApp(
      navigatorKey: _navigator,
      debugShowCheckedModeBanner: false,
      theme: orgBotTheme(Brightness.dark, desktop: true),
      home: _UiAcceptanceApp(
        key: _fixtureState,
        captureDir: captureDir,
        errors: errors,
      ),
      builder: (context, child) => RepaintBoundary(
        key: _captureBoundary,
        child: child ?? const SizedBox.shrink(),
      ),
    ),
  );
}

class _UiAcceptanceApp extends StatefulWidget {
  const _UiAcceptanceApp({
    super.key,
    required this.captureDir,
    required this.errors,
  });

  final Directory captureDir;
  final List<String> errors;

  @override
  State<_UiAcceptanceApp> createState() => _UiAcceptanceState();
}

class _UiAcceptanceState extends State<_UiAcceptanceApp>
    with WidgetsBindingObserver {
  late final _FixtureLink _link;
  late final RoomStore _store;
  var _capturing = false;
  var _capturedNarrow = false;

  static const _longConflict =
      'packages/orgbot_desktop/linux/src/workspace/agents/finn/reports/quarterly-release/readme-consolidation/review-notes.md';
  static const _longDeleted =
      'tooling/maintenance/archive/obsolete-provider-cache/generated-index-2024-10-26.json';
  static const _longAdded =
      'docs/operations/provider-usage/cache-read-and-cache-write-accounting.md';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _link = _FixtureLink();
    _store = RoomStore(_link, notificationRoomId: 'ui-acceptance-room');
    _store.restore([
      {
        'type': 'snapshot',
        'head': 0,
        'roster': const [],
        'tasks': const [],
        'digest': {
          'objectives': [
            {
              'task': 'release-cost-controls',
              'title': 'Keep release quality within the provider spend plan',
              'progress': {'done': 7, 'total': 11},
              'status': 'in progress',
              'owners': [
                'astra-security-review',
                'julian-architecture',
                'finn-cli',
              ],
              'records': ['work-1', 'work-2', 'work-3'],
            },
          ],
          'counts': {
            'objectives': 4,
            'active': 3,
            'waiting_internally': 1,
            'needs_you': 0,
            'delivered': 2,
            'raw_records': 19,
          },
          'needs_you': const [],
          'critical_path': const [],
          'health': const {},
          'utilization': const {'bots': [], 'idle': 0, 'unclaimed': 0},
          'spend': [
            {
              'backend': 'Codex CLI · subscription',
              'turns': 84,
              'tokens': 0,
              'per_turn': 0,
              'input_tokens': 1420000,
              'output_tokens': 186400,
              'cache_read_tokens': 8420000,
              'cache_write_tokens': 1130000,
              'usage_known': true,
              'api_equivalent_cost_microusd': 6420000,
            },
            {
              'backend': 'DeepSeek API · deepseek-v4.1-flash',
              'turns': 31,
              'tokens': 462800,
              'per_turn': 14929,
              'input_tokens': 402100,
              'output_tokens': 60700,
              'cache_read_tokens': 1740000,
              'cache_write_tokens': 182000,
              'usage_known': true,
              'actual_api_cost_microusd': 684000,
            },
          ],
          'agent_spend': [
            {
              'handle': 'astra-security-review',
              'turns': 17,
              'input_tokens': 244000,
              'output_tokens': 32100,
              'cache_read_tokens': 1520000,
              'cache_write_tokens': 218000,
              'usage_known': true,
              'api_equivalent_cost_microusd': 1110000,
            },
            {
              'handle': 'julian-architecture-crosscheck',
              'turns': 14,
              'input_tokens': 392000,
              'output_tokens': 48800,
              'cache_read_tokens': 2390000,
              'cache_write_tokens': 302000,
              'usage_known': true,
              'api_equivalent_cost_microusd': 1840000,
            },
            {
              'handle': 'finn-cli-implementation-tests',
              'turns': 22,
              'input_tokens': 318000,
              'output_tokens': 40700,
              'cache_read_tokens': 2070000,
              'cache_write_tokens': 264000,
              'usage_known': true,
              'actual_api_cost_microusd': 246000,
            },
            {
              'handle': 'devon-deepseek-builder',
              'turns': 19,
              'input_tokens': 221000,
              'output_tokens': 28400,
              'cache_read_tokens': 1280000,
              'cache_write_tokens': 198000,
              'usage_known': true,
              'actual_api_cost_microusd': 438000,
            },
          ],
          'total_spend': {
            'turns': 115,
            'input_tokens': 1822100,
            'output_tokens': 247100,
            'cache_read_tokens': 10160000,
            'cache_write_tokens': 1312000,
            'usage_known': true,
            'api_equivalent_cost_microusd': 7104000,
          },
        },
      },
    ]);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(_captureWidth('wide'));
    });
  }

  @override
  void didChangeMetrics() {
    super.didChangeMetrics();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _capturedNarrow || _capturing) return;
      final context = _captureBoundary.currentContext;
      if (context == null) return;
      final width = MediaQuery.sizeOf(context).width;
      if (width >= 1010 && width <= 1035) {
        _capturedNarrow = true;
        unawaited(_captureWidth('narrow'));
      }
    });
  }

  Future<void> _captureWidth(String label) async {
    if (_capturing || !mounted) return;
    _capturing = true;
    try {
      await Future<void>.delayed(const Duration(milliseconds: 700));
      await _capture('$label-usage');
      await _showWorkspace('conflict');
      await Future<void>.delayed(const Duration(milliseconds: 250));
      await _capture('$label-workspace-conflicts');
      _navigator.currentState?.pop();
      await Future<void>.delayed(const Duration(milliseconds: 250));

      await _showWorkspace('deletion');
      await Future<void>.delayed(const Duration(milliseconds: 250));
      await _capture('$label-workspace-deletions');
      await _tapText('Apply changes');
      await Future<void>.delayed(const Duration(milliseconds: 250));
      await _capture('$label-workspace-delete-approval');
      await _tapText('Cancel');
      await Future<void>.delayed(const Duration(milliseconds: 200));
      _navigator.currentState?.pop();
      await Future<void>.delayed(const Duration(milliseconds: 250));

      if (label == 'wide') {
        await File(
          '${widget.captureDir.path}/ready-1280',
        ).writeAsString('ready');
      } else {
        await _writeReport(label);
        await Future<void>.delayed(const Duration(milliseconds: 300));
        exit(widget.errors.isEmpty ? 0 : 2);
      }
    } catch (error) {
      widget.errors.add('capture $label failed: $error');
      await _writeReport(label);
      await Future<void>.delayed(const Duration(milliseconds: 200));
      exit(2);
    } finally {
      _capturing = false;
    }
  }

  Future<void> _capture(String name) async {
    await WidgetsBinding.instance.endOfFrame;
    final context = _captureBoundary.currentContext;
    final boundary = context?.findRenderObject() as RenderRepaintBoundary?;
    if (boundary == null || boundary.debugNeedsPaint) {
      throw StateError('Capture surface was not painted for $name');
    }
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      if (bytes == null) throw StateError('PNG encoding failed for $name');
      await File(
        '${widget.captureDir.path}/$name.png',
      ).writeAsBytes(bytes.buffer.asUint8List(), flush: true);
      _captureInfo[name] = {'width': image.width, 'height': image.height};
    } finally {
      image.dispose();
    }
  }

  final Map<String, Object?> _captureInfo = {};

  Future<void> _writeReport(String label) async {
    final context = _captureBoundary.currentContext;
    await File('${widget.captureDir.path}/report-$label.json').writeAsString(
      const JsonEncoder.withIndent('  ').convert({
        'label': label,
        'viewport_width': context == null
            ? null
            : MediaQuery.sizeOf(context).width,
        'viewport_height': context == null
            ? null
            : MediaQuery.sizeOf(context).height,
        'screenshots': _captureInfo,
        'flutter_errors': widget.errors,
        'fixture_data_only': true,
      }),
      flush: true,
    );
  }

  Map<String, Object?> _workspaceStatus(String scenario) {
    final common = <String, Object?>{
      'ok': true,
      'workspace': '/tmp/orgbot-ui-fixture/workspaces/project/finn',
      'added': [_longAdded],
      'deleted': [_longDeleted],
    };
    if (scenario == 'deletion') {
      common['changed'] = const [];
      common['conflicts'] = const [];
    } else {
      common['changed'] = [
        {'path': _longConflict, 'kind': 'modified'},
      ];
      common['conflicts'] = [_longConflict];
    }
    return common;
  }

  Future<void> _showWorkspace(String scenario) async {
    final future = showDialog<void>(
      context: _navigator.currentContext!,
      builder: (context) => WorkspaceChangesDialog(
        stateDirectory: Directory('/tmp/orgbot-ui-fixture/workspaces'),
        agents: const [
          WorkspaceAgentLabel(
            id: 'finn-agent-fixture',
            label: 'Finn · CLI implementation and test evidence',
          ),
        ],
        helperPath: '/fixture/orgbot-workspace.py',
        processRunner: (executable, arguments) async {
          final result = _workspaceStatus(scenario);
          return ProcessResult(0, 0, jsonEncode(result), '');
        },
      ),
    );
    await Future<void>.delayed(const Duration(milliseconds: 300));
    // The dialog route is dismissed by the calling capture sequence.
    unawaited(future);
  }

  Future<void> _tapText(String label) async {
    final root = _navigator.currentContext;
    if (root == null) return;
    Element? target;
    void visit(Element element) {
      if (target != null) return;
      final widget = element.widget;
      if (widget is Text && widget.data == label) {
        target = element;
        return;
      }
      element.visitChildElements(visit);
    }

    (root as Element).visitChildElements(visit);
    final box = target?.renderObject;
    if (box is! RenderBox || !box.hasSize) {
      throw StateError(
        'Could not find tappable "$label" in the rendered dialog.',
      );
    }
    final point = box.localToGlobal(box.size.center(Offset.zero));
    GestureBinding.instance.handlePointerEvent(
      PointerDownEvent(pointer: 731, position: point, buttons: kPrimaryButton),
    );
    await Future<void>.delayed(const Duration(milliseconds: 50));
    GestureBinding.instance.handlePointerEvent(
      PointerUpEvent(pointer: 731, position: point),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Room usage'),
        actions: [
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 12, vertical: 18),
            child: Text('Seeded provider reports · no live account'),
          ),
          IconButton(
            tooltip: 'Review workspace changes',
            onPressed: () => _showWorkspace('conflict'),
            icon: const Icon(Icons.merge_type),
          ),
        ],
      ),
      body: Align(
        alignment: Alignment.topCenter,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 1000),
          child: ObjectivesPane(store: _store),
        ),
      ),
    );
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _store.dispose();
    unawaited(_link.close());
    super.dispose();
  }
}
