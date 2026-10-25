import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/desktop_integrations.dart';
import 'package:orgbot_app/orgbot/integration_registry.dart';
import 'package:orgbot_app/ui/desktop_integrations_view.dart';
import 'package:orgbot_app/ui/desktop_project_pane.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('unselected project pane has no empty preview region', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(390, 780);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(body: DesktopProjectPane(projectPath: null)),
      ),
    );
    expect(find.text('Choose folder'), findsOneWidget);
    expect(find.text('No internal preview'), findsNothing);
  });

  testWidgets(
    'stale Git requests cannot replace output or clear newer loading',
    (tester) async {
      final requests = <String, Completer<String>>{};
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: DesktopProjectPane(
              projectPath: null,
              initialRootForTesting: '/test/project',
              gitOutputForTesting: (root, args) {
                final view = args.first == 'log'
                    ? 'history'
                    : args.first == 'diff'
                    ? 'diff'
                    : 'status';
                final request = Completer<String>();
                requests[view] = request;
                return request.future;
              },
            ),
          ),
        ),
      );

      await tester.tap(find.text('Git'));
      await tester.pump();
      expect(requests.containsKey('status'), isTrue);
      await tester.tap(find.text('History'));
      await tester.pump();
      expect(requests.containsKey('history'), isTrue);

      requests['status']!.completeError(StateError('stale status failure'));
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsOneWidget);
      expect(find.textContaining('stale status failure'), findsNothing);

      requests['history']!.complete('new history output');
      await tester.pump();
      expect(find.text('new history output'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
    },
  );

  testWidgets('integration sections start collapsed and skills build lazily', (
    tester,
  ) async {
    late Directory home;
    late DesktopIntegrationSnapshot snapshot;
    await tester.runAsync(() async {
      home = await Directory.systemTemp.createTemp('orgbot-tools-ui-');
      final skills = Directory('${home.path}/.agents/skills');
      await skills.create(recursive: true);
      for (var i = 0; i < 180; i++) {
        await Directory(
          '${skills.path}/skill-${i.toString().padLeft(3, '0')}',
        ).create();
        await File(
          '${skills.path}/skill-${i.toString().padLeft(3, '0')}/SKILL.md',
        ).writeAsString('fixture');
      }
      snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
        registry: IntegrationRegistry(
          supportDirectory: '${home.path}/.local/share/orgbot',
        ),
      );
    });
    addTearDown(() async => home.delete(recursive: true));

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: DesktopIntegrationsView(
              homeDirectory: home.path,
              discoverForTesting: () async => snapshot,
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Skills'), findsOneWidget);
    expect(find.text('180'), findsOneWidget);
    expect(find.text('skill-000'), findsNothing);
    await tester.tap(find.text('Skills'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('skill-000'), findsOneWidget);
    expect(find.text('skill-179'), findsNothing);

    await tester.runAsync(() async {
      await Directory('${home.path}/.codex').create(recursive: true);
      await File('${home.path}/.codex/config.toml').writeAsString('''
[mcp_servers.local]
command = "node"
''');
      snapshot = await DesktopIntegrations.discover(
        homeDirectory: home.path,
        registry: IntegrationRegistry(
          supportDirectory: '${home.path}/.local/share/orgbot',
        ),
      );
    });
    await tester.tap(find.byTooltip('Refresh tools'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('1'), findsOneWidget);
    await tester.tap(find.text('MCP servers'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Configured'), findsOneWidget);
  });
}
