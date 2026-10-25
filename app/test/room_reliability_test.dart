import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';
import 'package:orgbot_app/ui/room_widgets.dart';
import 'package:orgbot_app/workspace/workspace.dart';
import 'package:orgbot_app/workspace/workspace_screen.dart';

void main() {
  tearDown(() {
    ModelCatalog.credentials = null;
    ModelCatalog.invalidate();
  });
  testWidgets('the displayed model and user profile are actually saved', (
    tester,
  ) async {
    FlutterSecureStorage.setMockInitialValues({});
    SharedPreferences.setMockInitialValues({});
    final credentials = Credentials();
    await credentials.load();
    await credentials.importBundle(
      jsonEncode({
        'version': 1,
        'providers': [
          {
            'provider': 'codex',
            'token': 'test-token',
            'models': [
              {'id': 'account-model', 'label': 'Account model'},
            ],
          },
        ],
      }),
    );
    ModelCatalog.credentials = credentials;
    final library = Library(saver: MemorySaver());
    await library.load();
    library.addBot(
      BotDef(name: 'Iris', handle: 'iris', provider: ProviderKind.codex),
    );
    await tester.pumpWidget(
      MaterialApp(home: OrgBotScreen(library: library, desktopClis: false)),
    );
    await tester.tap(find.text('Bots'));
    await tester.pumpAndSettle();
    await tester.tap(find.textContaining('@iris'));
    await tester.pumpAndSettle();
    expect(find.text('Account model'), findsWidgets);
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(library.bots.single.model, 'account-model');
    await tester.tap(find.byTooltip('Your profile'));
    await tester.pumpAndSettle();
    final fields = find.descendant(
      of: find.byType(AlertDialog),
      matching: find.byType(TextField),
    );
    await tester.enterText(fields.at(0), 'Alex');
    await tester.enterText(
      fields.at(1),
      'Founder. Ask before choosing geography.',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final reloaded = Library(saver: library.saver);
    await tester.runAsync(reloaded.load);
    expect(reloaded.userName, 'Alex');
    expect(reloaded.userProfile, contains('geography'));
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
  });
  testWidgets(
    'connection save commits before returning and tags insert at the cursor',
    (tester) async {
      final dir = (await tester.runAsync(
        () => Directory.systemTemp.createTemp('orgbot-save-check-'),
      ))!;
      final library = Library(saver: MemorySaver());
      await library.load();
      final room = library.addRoom(RoomDef(name: 'saved room'));
      try {
        await tester.pumpWidget(
          MaterialApp(
            home: Builder(
              builder: (context) => TextButton(
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute(
                    builder: (_) => WorkspaceScreen(
                      config: WorkspaceConfig(path: dir.path),
                      roomId: room.id,
                      onSave: (config) async {
                        room.workspace = config.copy();
                        await library.save();
                      },
                    ),
                  ),
                ),
                child: const Text('Open workspace'),
              ),
            ),
          ),
        );
        await tester.tap(find.text('Open workspace'));
        await tester.pumpAndSettle();
        await tester.runAsync(() async {
          await tester.tap(find.text('Test connection and save'));
          for (var i = 0; i < 100 && room.workspace.path != dir.path; i++) {
            await Future<void>.delayed(const Duration(milliseconds: 10));
          }
        });
        await tester.pumpAndSettle();
        final loaded = Library(saver: library.saver);
        await tester.runAsync(loaded.load);
        expect(loaded.rooms.single.workspace.path, dir.path);
        final text = TextEditingController(text: 'Please check');
        text.selection = const TextSelection.collapsed(offset: 0);
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: Composer(
                controller: text,
                onSend: () {},
                mentions: const ['iris', 'teo'],
              ),
            ),
          ),
        );
        await tester.tap(find.byTooltip('Add to message'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('@iris'));
        await tester.pumpAndSettle();
        expect(text.text, '@iris Please check');
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pumpAndSettle();
        text.dispose();
      } finally {
        await tester.runAsync(() => dir.delete(recursive: true));
      }
    },
  );
}
