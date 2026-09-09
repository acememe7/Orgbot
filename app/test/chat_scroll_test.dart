import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/main.dart';
import 'package:orgbot_app/ui/room_widgets.dart';
import 'room_chat_ui_test.dart' show FakeClient, seeded, event;

void main() {
  testWidgets(
    'history stays anchored through arrivals, status changes and dragging',
    (tester) async {
      tester.view.physicalSize = const Size(390, 780);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final client = FakeClient();
      final store = seeded(client);
      final scroll = ScrollController();
      addTearDown(store.dispose);
      addTearDown(scroll.dispose);
      var seq = 0;
      final posted = <int, String>{};
      void post(int id, {String? body}) {
        posted[id] =
            body ??
            posted[id] ??
            'Message $id\n\n${List.filled(id % 5 + 1, 'A variable height paragraph with readable content.').join('\n\n')}';
        client.emit(
          event({
            'event': 'message_posted',
            'message': {
              'id': 'm$id',
              'room': 'r',
              'author': {'kind': 'agent', 'agent': 'bot_a'},
              'audience': {'kind': 'everyone'},
              'intent': 'say',
              'sent_at': 0,
              'body': posted[id],
            },
          }, seq: ++seq),
        );
      }

      for (var i = 0; i < 80; i++) {
        post(i);
      }
      await tester.pumpWidget(
        MaterialApp(
          theme: orgBotTheme(Brightness.dark),
          home: Scaffold(
            body: ListenableBuilder(
              listenable: store,
              builder: (_, _) => Transcript(store: store, controller: scroll),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final view = find.byType(CustomScrollView);
      await tester.drag(view, const Offset(0, 550));
      await tester.pumpAndSettle();
      expect(find.text('Jump to latest'), findsOneWidget);
      // Use a fully visible message, independent of the virtualizer's cache.
      final visible = List.generate(80, (i) => find.byKey(ValueKey('m$i')))
          .where((f) {
            if (f.evaluate().isEmpty) return false;
            final r = tester.getRect(f);
            return r.top >= 0 && r.top < 600;
          })
          .first;
      final y = tester.getTopLeft(visible).dy;
      for (var i = 80; i < 84; i++) {
        post(
          i,
          body:
              '# Audit $i\n\n```dart\n${List.filled(12, 'final value = 42;').join('\n')}\n```',
        );
      }
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(visible).dy, closeTo(y, 1));
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'text_delta', 'text': 'Working'},
      });
      await tester.pump();
      expect(tester.getTopLeft(visible).dy, closeTo(y, 1));
      client.emit(
        event({
          'event': 'agent_errored',
          'agent': 'bot_a',
          'error': 'Network unavailable',
          'attempt': 1,
        }, seq: ++seq),
      );
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(visible).dy, closeTo(y, 1));
      // A reconnect's snapshot and journal can arrive across separate frames.
      client.emit({'type': 'snapshot', 'head': seq + posted.length});
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(visible).dy, closeTo(y, 1));
      for (final id in posted.keys.toList()) {
        post(id);
        await tester.pump();
        expect(tester.getTopLeft(visible).dy, closeTo(y, 1));
      }
      await tester.pumpAndSettle();
      // A mounted markdown row changes height without changing its identity.
      final anchorId = int.parse(
        (tester.widget(visible).key as ValueKey<String>).value.substring(1),
      );
      final changed = anchorId + 1;
      posted[changed] =
          'Expanded audit\n\n${List.filled(12, 'More audit detail.').join('\n\n')}';
      client.emit({'type': 'snapshot', 'head': 0});
      for (final id in posted.keys.toList()) {
        post(id);
      }
      await tester.pumpAndSettle();
      expect(
        tester.getTopLeft(visible).dy,
        closeTo(y, 1),
        reason: 'resizing a mounted neighbor must retain the reading anchor',
      );
      // Hold a real drag while more messages arrive; no pending follow can win.
      final gesture = await tester.startGesture(tester.getCenter(view));
      await gesture.moveBy(const Offset(0, 80));
      await tester.pump();
      final dragOffset = scroll.offset;
      post(84);
      await tester.pump(const Duration(milliseconds: 300));
      expect(scroll.offset, closeTo(dragOffset, 1));
      await gesture.up();
      await tester.pumpAndSettle();
      await tester.tap(find.text('Jump to latest'));
      await tester.pumpAndSettle();
      expect(scroll.position.extentAfter, closeTo(0, 1));
      post(85);
      await tester.pumpAndSettle();
      expect(scroll.position.extentAfter, closeTo(0, 1));
      expect(find.byKey(const ValueKey('m85')), findsOneWidget);
      // Wheel input is also an explicit historical-reading gesture.
      final pointer = TestPointer(1, PointerDeviceKind.mouse);
      tester.binding.handlePointerEvent(pointer.hover(tester.getCenter(view)));
      tester.binding.handlePointerEvent(pointer.scroll(const Offset(0, -450)));
      await tester.pumpAndSettle();
      final wheelOffset = scroll.offset;
      post(86);
      await tester.pumpAndSettle();
      expect(scroll.offset, closeTo(wheelOffset, 1));
      // Exercise the actual scrollbar thumb, including an arrival mid-drag.
      tester.binding.handlePointerEvent(pointer.scroll(const Offset(0, -20)));
      await tester.pump(const Duration(milliseconds: 100));
      await tester.pump(const Duration(milliseconds: 100));
      final painters = tester
          .widgetList<CustomPaint>(
            find.descendant(
              of: find.byType(Scrollbar),
              matching: find.byType(CustomPaint),
            ),
          )
          .map((w) => w.foregroundPainter)
          .whereType<ScrollbarPainter>()
          .toList();
      final thumb =
          [
                for (var y = 0; y < 760; y++)
                  for (var x = 376; x < 390; x++)
                    Offset(x.toDouble(), y.toDouble()),
              ]
              .where(
                (p) => painters.any(
                  (painter) => painter.hitTestOnlyThumbInteractive(
                    p,
                    PointerDeviceKind.mouse,
                  ),
                ),
              )
              .toList();
      expect(thumb, isNotEmpty);
      final mouse = await tester.startGesture(
        thumb[thumb.length ~/ 2],
        kind: PointerDeviceKind.mouse,
      );
      await mouse.moveBy(const Offset(0, -100));
      await tester.pump();
      final thumbOffset = scroll.offset;
      expect(thumbOffset, lessThan(wheelOffset));
      post(87);
      await tester.pump(const Duration(milliseconds: 300));
      expect(scroll.offset, closeTo(thumbOffset, 1));
      await mouse.up();
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );
}
