import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/widgets/memory_pane.dart';

ProjectMemoryItem item({bool shared = true}) => ProjectMemoryItem(
  id: 'entry-1',
  title: 'Restart recovery',
  content: 'Persist login state after application restart.',
  sourceRoom: 'Release',
  sourceTask: 'task_1',
  shared: shared,
);

Widget pane({
  required List<ProjectMemoryItem> entries,
  required Future<void> Function(String, bool) onSetShared,
  required Future<void> Function(String) onDelete,
  required Future<void> Function(String, String) onShare,
  required Future<void> Function(String) onSearch,
}) => MaterialApp(
  home: Scaffold(
    body: MemoryPane(
      entries: entries,
      onSetShared: onSetShared,
      onDelete: onDelete,
      onShareToProject: onShare,
      onSearch: onSearch,
    ),
  ),
);

void main() {
  testWidgets('memory sharing is explicit and scoped to a target project', (
    tester,
  ) async {
    String? sharedId;
    String? targetProject;
    bool? visibility;
    await tester.pumpWidget(
      pane(
        entries: [item()],
        onSetShared: (id, shared) async {
          sharedId = id;
          visibility = shared;
        },
        onDelete: (_) async {},
        onShare: (id, target) async {
          sharedId = id;
          targetProject = target;
        },
        onSearch: (_) async {},
      ),
    );

    await tester.tap(find.byType(Switch));
    await tester.pump();
    expect(sharedId, 'entry-1');
    expect(visibility, isFalse);

    await tester.tap(find.text('Share copy'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'project-beta');
    await tester.tap(find.text('Share copy').last);
    await tester.pumpAndSettle();
    expect(targetProject, 'project-beta');
  });

  testWidgets(
    'delete requires owner confirmation and search delegates to local store',
    (tester) async {
      String? deleted;
      String? query;
      await tester.pumpWidget(
        pane(
          entries: [item(shared: false)],
          onSetShared: (_, _) async {},
          onDelete: (id) async {
            deleted = id;
          },
          onShare: (_, _) async {},
          onSearch: (value) async {
            query = value;
          },
        ),
      );

      await tester.tap(find.byTooltip('Delete memory'));
      await tester.pumpAndSettle();
      expect(deleted, isNull);
      await tester.tap(find.text('Delete').last);
      await tester.pumpAndSettle();
      expect(deleted, 'entry-1');

      await tester.enterText(
        find.byType(TextField),
        'restore authentication after reboot',
      );
      await tester.tap(find.text('Search'));
      await tester.pumpAndSettle();
      expect(query, 'restore authentication after reboot');
    },
  );
}
