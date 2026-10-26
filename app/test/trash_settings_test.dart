import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/trash_storage_settings.dart';
import 'package:orgbot_app/workspace/desktop_files.dart';
import 'package:orgbot_app/workspace/trash_settings.dart';

ProcessResult result(String stdout, {int exitCode = 0, String stderr = ''}) =>
    ProcessResult(1, exitCode, stdout, stderr);

String response({
  String root = '/home/test/OrgBot Trash',
  String source = 'configured',
  bool requiresSelection = false,
}) =>
    '{"root":"$root","source":"$source","requires_selection":$requiresSelection}';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('TrashSettingsClient', () {
    test(
      'sends config and configure as argument lists without a shell',
      () async {
        final calls = <List<String>>[];
        final client = TrashSettingsClient(
          helperPath: '/isolated/orgbot-trash.py',
          processRunner: (executable, arguments) async {
            expect(executable, 'python3');
            calls.add(arguments);
            return result(response());
          },
        );

        final current = await client.read();
        final saved = await client.configure('/tmp/recovery folder');

        expect(current.root, '/home/test/OrgBot Trash');
        expect(saved.source, 'configured');
        expect(calls, [
          ['/isolated/orgbot-trash.py', 'config'],
          [
            '/isolated/orgbot-trash.py',
            'configure',
            '--root',
            '/tmp/recovery folder',
          ],
        ]);
      },
    );

    test('rejects malformed JSON, wrong fields, and helper stderr', () async {
      for (final output in [
        '{not json',
        '{"root":"/tmp","source":"configured","requires_selection":"no"}',
      ]) {
        final client = TrashSettingsClient(
          helperPath: '/fake/helper.py',
          processRunner: (_, _) async => result(output),
        );
        await expectLater(client.read(), throwsFormatException);
      }

      final failed = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, _) async =>
            result('', exitCode: 2, stderr: 'Recovery folder is not empty.'),
      );
      await expectLater(
        failed.configure('/tmp/new'),
        throwsA(
          isA<FormatException>().having(
            (error) => error.message,
            'message',
            'Recovery folder is not empty.',
          ),
        ),
      );
    });

    test('requires an absolute folder', () async {
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, _) async => result(response()),
      );
      expect(() => client.configure('relative/path'), throwsFormatException);
    });

    test('times out a stalled helper with an actionable error', () async {
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        timeout: const Duration(milliseconds: 10),
        processRunner: (_, _) => Completer<ProcessResult>().future,
      );
      await expectLater(
        client.read(),
        throwsA(
          isA<FormatException>().having(
            (error) => error.message,
            'message',
            contains('timed out'),
          ),
        ),
      );
    });

    test(
      'kills a real temporary Python helper when it exceeds its deadline',
      () async {
        final temp = await Directory.systemTemp.createTemp(
          'orgbot-trash-timeout-',
        );
        addTearDown(() => temp.delete(recursive: true));
        final helper = File('${temp.path}/slow-helper.py');
        await helper.writeAsString('import time\ntime.sleep(30)\n');
        final client = TrashSettingsClient(
          helperPath: helper.path,
          timeout: const Duration(milliseconds: 100),
        );
        final stopwatch = Stopwatch()..start();
        await expectLater(client.read(), throwsFormatException);
        expect(stopwatch.elapsed, lessThan(const Duration(seconds: 4)));
      },
    );

    test('kills a helper that exceeds the output cap', () async {
      final temp = await Directory.systemTemp.createTemp(
        'orgbot-trash-output-',
      );
      addTearDown(() => temp.delete(recursive: true));
      final helper = File('${temp.path}/noisy-helper.py');
      await helper.writeAsString(
        'import sys\nsys.stdout.write("x" * (65 * 1024))\n',
      );
      final client = TrashSettingsClient(helperPath: helper.path);
      await expectLater(
        client.read(),
        throwsA(
          isA<FormatException>().having(
            (error) => error.message,
            'message',
            contains('too much output'),
          ),
        ),
      );
    });
  });

  group('TrashStorageSettings', () {
    testWidgets(
      'shows suggested legacy folder and keeps edits after save fails',
      (tester) async {
        final calls = <List<String>>[];
        final client = TrashSettingsClient(
          helperPath: '/fake/helper.py',
          processRunner: (_, arguments) async {
            calls.add(arguments);
            if (arguments.length == 2) {
              return result(
                response(
                  source: 'policy-requires-selection',
                  requiresSelection: true,
                ),
              );
            }
            return result('', exitCode: 2, stderr: 'Choose an empty folder.');
          },
        );

        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(body: TrashStorageSettings(client: client)),
          ),
        );
        await tester.pumpAndSettle();
        expect(
          find.byKey(const ValueKey('trash-selection-needed')),
          findsOneWidget,
        );
        final field = tester.widget<TextField>(
          find.byKey(const ValueKey('trash-root-field')),
        );
        expect(field.controller!.text, '/home/test/OrgBot Trash');

        await tester.enterText(
          find.byKey(const ValueKey('trash-root-field')),
          '/home/test/new folder',
        );
        await tester.tap(find.byKey(const ValueKey('trash-save')));
        await tester.pumpAndSettle();
        expect(find.text('Choose an empty folder.'), findsOneWidget);
        expect(
          tester
              .widget<TextField>(find.byKey(const ValueKey('trash-root-field')))
              .controller!
              .text,
          '/home/test/new folder',
        );
        expect(calls.last, [
          '/fake/helper.py',
          'configure',
          '--root',
          '/home/test/new folder',
        ]);
      },
    );

    testWidgets('shows loading, disables actions, and can retry', (
      tester,
    ) async {
      final readResult = Completer<ProcessResult>();
      var reads = 0;
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, _) {
          reads++;
          if (reads == 1) return readResult.future;
          return Future.value(result(response()));
        },
      );

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: TrashStorageSettings(client: client)),
        ),
      );
      await tester.pump();
      expect(find.text('Checking recovery folder…'), findsOneWidget);
      readResult.complete(result('not-json'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('trash-error')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('trash-retry')));
      await tester.pumpAndSettle();
      expect(
        find.text('Current folder: /home/test/OrgBot Trash'),
        findsOneWidget,
      );
    });

    testWidgets('disables editing while a save is in flight', (tester) async {
      final saving = Completer<ProcessResult>();
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, arguments) => arguments.length == 2
            ? Future.value(result(response()))
            : saving.future,
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: TrashStorageSettings(client: client)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('trash-root-field')),
        '/home/test/next',
      );
      await tester.tap(find.byKey(const ValueKey('trash-save')));
      await tester.pump();
      expect(find.text('Saving…'), findsOneWidget);
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('trash-root-field')))
            .enabled,
        isFalse,
      );
      expect(
        tester
            .widget<FilledButton>(find.byKey(const ValueKey('trash-save')))
            .onPressed,
        isNull,
      );
      saving.complete(result(response(root: '/home/test/next')));
      await tester.pumpAndSettle();
      expect(find.text('Current folder: /home/test/next'), findsOneWidget);
    });

    testWidgets('uses the native directory chooser and remains narrow-safe', (
      tester,
    ) async {
      tester.view.physicalSize = const Size(360, 600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(desktopWorkspaceChannel, (call) async {
        expect(call.method, 'choose_directory');
        return '/home/test/selected';
      });
      addTearDown(
        () => messenger.setMockMethodCallHandler(desktopWorkspaceChannel, null),
      );
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, _) async => result(response()),
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: TrashStorageSettings(client: client)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('trash-browse')));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('trash-root-field')))
            .controller!
            .text,
        '/home/test/selected',
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('captures normal and narrow recovery settings renders', (
      tester,
    ) async {
      final font = Platform.environment['ORGBOT_UI_FONT'];
      if (font != null) {
        await tester.runAsync(() async {
          final icons = File(
            '${File(font).parent.path}/MaterialIcons-Regular.otf',
          );
          if (await icons.exists()) {
            await (FontLoader('MaterialIcons')..addFont(
                  Future.value(ByteData.sublistView(await icons.readAsBytes())),
                ))
                .load();
          }
          await (FontLoader('Roboto')..addFont(
                Future.value(
                  ByteData.sublistView(await File(font).readAsBytes()),
                ),
              ))
              .load();
        });
      }
      final client = TrashSettingsClient(
        helperPath: '/fake/helper.py',
        processRunner: (_, _) async => result(response()),
      );
      for (final (width, path) in [
        (900.0, '/tmp/orgbots-trash-settings-normal.png'),
        (360.0, '/tmp/orgbots-trash-settings-narrow.png'),
      ]) {
        tester.view.physicalSize = Size(width, 760);
        tester.view.devicePixelRatio = 1;
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: Center(
                child: RepaintBoundary(
                  key: const ValueKey('trash-capture'),
                  child: SizedBox(
                    width: width - 32,
                    child: TrashStorageSettings(client: client),
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.runAsync(() async {
          final boundary = tester.renderObject<RenderRepaintBoundary>(
            find.byKey(const ValueKey('trash-capture')),
          );
          final image = await boundary.toImage(pixelRatio: 1);
          final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
          await File(path).writeAsBytes(bytes!.buffer.asUint8List());
          image.dispose();
        });
      }
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });
  });
}
