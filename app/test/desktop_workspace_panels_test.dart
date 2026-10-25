import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/desktop_project_pane.dart';
import 'package:orgbot_app/ui/workspace_native_surface.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('web preview opens only HTTP(S) and terminal uses chosen root', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(nativeWorkspacePanelChannel, (call) async {
          calls.add(call);
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(nativeWorkspacePanelChannel, null),
    );
    late Directory root;
    await tester.runAsync(() async {
      root = await Directory.systemTemp.createTemp('orgbot-panels-');
    });
    addTearDown(() async => root.delete(recursive: true));

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 900,
            height: 620,
            child: DesktopProjectPane(
              projectPath: null,
              initialRootForTesting: root.path,
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.byTooltip('Web preview'));
    await tester.pump();
    await tester.enterText(find.byType(TextField), 'example.com/path');
    await tester.tap(find.byTooltip('Open address'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(calls.any((call) => call.method == 'browser_show'), isTrue);
    final browserCall = calls.lastWhere(
      (call) => call.method == 'browser_show',
    );
    expect((browserCall.arguments as Map)['uri'], 'https://example.com/path');

    await tester.enterText(find.byType(TextField), 'javascript:alert(1)');
    await tester.tap(find.byTooltip('Open address'));
    await tester.pump();
    expect(find.text('Enter a valid HTTP or HTTPS address.'), findsOneWidget);
    final browserCalls = calls.where((call) => call.method == 'browser_show');
    expect(browserCalls, isNotEmpty);
    expect(
      browserCalls.every(
        (call) => (call.arguments as Map)['uri'] == 'https://example.com/path',
      ),
      isTrue,
    );

    await tester.tap(find.byTooltip('Local terminal'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    final terminalCall = calls.lastWhere(
      (call) => call.method == 'terminal_show',
    );
    expect((terminalCall.arguments as Map)['root'], root.path);
    await tester.tap(find.byTooltip('Stop terminal process group'));
    await tester.pump();
    expect(calls.any((call) => call.method == 'terminal_stop'), isTrue);
    await tester.tap(find.text('Files'));
    await tester.pump();
    expect(find.byType(NativeWorkspaceSurface), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'media surface transfers bounded-preview payload to native player',
    (tester) async {
      tester.view.physicalSize = const Size(1200, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(nativeWorkspacePanelChannel, (call) async {
            calls.add(call);
            return null;
          });
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(nativeWorkspacePanelChannel, null),
      );
      final bytes = Uint8List.fromList([1, 2, 3, 4]);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 900,
              height: 600,
              child: _MediaSurfaceFixture(bytes: bytes),
            ),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 100));
      final call = calls.singleWhere(
        (call) => call.method == 'browser_media_show',
      );
      final arguments = call.arguments as Map;
      expect(arguments['bytes'], orderedEquals(bytes));
      expect(arguments['kind'], 'video');
      expect(arguments['extension'], 'mp4');
    },
  );

  testWidgets('native surfaces yield to dialogs and restore after dismissal', (
    tester,
  ) async {
    final calls = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(nativeWorkspacePanelChannel, (call) async {
          calls.add(call.method);
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(nativeWorkspacePanelChannel, null),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Column(
              children: [
                TextButton(
                  onPressed: () => showDialog<void>(
                    context: context,
                    builder: (context) => AlertDialog(
                      title: const Text('Owner control'),
                      actions: [
                        TextButton(
                          onPressed: () => Navigator.pop(context),
                          child: const Text('Close'),
                        ),
                      ],
                    ),
                  ),
                  child: const Text('Open dialog'),
                ),
                const Expanded(
                  child: NativeWorkspaceSurface(
                    panel: NativeWorkspacePanel.browser,
                    uri: 'https://example.com',
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(calls.last, 'browser_show');
    await tester.tap(find.text('Open dialog'));
    await tester.pumpAndSettle();
    expect(calls.last, 'browser_hide');
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
    expect(calls.last, 'browser_show');
  });

  testWidgets('native overlay hides when its Flutter tab is removed', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 700);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(nativeWorkspacePanelChannel, (call) async {
          calls.add(call);
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(nativeWorkspacePanelChannel, null),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: const SizedBox(
            width: 500,
            height: 400,
            child: NativeWorkspaceSurface(
              panel: NativeWorkspacePanel.browser,
              uri: 'https://example.com',
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 100));
    expect(calls.any((call) => call.method == 'browser_show'), isTrue);

    await tester.pumpWidget(
      const MaterialApp(home: Scaffold(body: SizedBox.shrink())),
    );
    await tester.pump();
    expect(calls.any((call) => call.method == 'browser_hide'), isTrue);
  });

  testWidgets('cancelled hidden media does not replace the restored preview', (
    tester,
  ) async {
    final pending = <Completer<void>>[];
    final errors = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(nativeWorkspacePanelChannel, (call) async {
          if (call.method == 'browser_media_show') {
            final request = Completer<void>();
            pending.add(request);
            await request.future;
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(nativeWorkspacePanelChannel, null),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Column(
              children: [
                TextButton(
                  onPressed: () => showDialog<void>(
                    context: context,
                    builder: (context) => AlertDialog(
                      actions: [
                        TextButton(
                          onPressed: () => Navigator.pop(context),
                          child: const Text('Close'),
                        ),
                      ],
                    ),
                  ),
                  child: const Text('Open dialog'),
                ),
                Expanded(
                  child: NativeWorkspaceSurface(
                    panel: NativeWorkspacePanel.browser,
                    mediaBytes: Uint8List.fromList([1, 2, 3]),
                    mediaKind: 'video',
                    mediaExtension: 'mp4',
                    mediaKey: 'sample',
                    onError: errors.add,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(pending, hasLength(1));
    await tester.tap(find.text('Open dialog'));
    await tester.pumpAndSettle();
    pending.first.completeError(
      PlatformException(code: 'cancelled', message: 'Media preview changed.'),
    );
    await tester.pumpAndSettle();
    expect(errors, isEmpty);
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
    expect(pending, hasLength(2));
    pending.last.completeError(
      PlatformException(code: 'read', message: 'Could not read media.'),
    );
    await tester.pumpAndSettle();
    expect(errors, ['Could not read media.']);
  });
}

class _MediaSurfaceFixture extends StatelessWidget {
  const _MediaSurfaceFixture({required this.bytes});

  final Uint8List bytes;

  @override
  Widget build(BuildContext context) => NativeWorkspaceSurface(
    panel: NativeWorkspacePanel.browser,
    mediaBytes: bytes,
    mediaKind: 'video',
    mediaExtension: 'mp4',
    mediaKey: 'sample.mp4:4:1',
  );
}
