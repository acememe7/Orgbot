// This opt-in smoke test wires the installed Linux plugin internals to a
// private synthetic Notifications service; production code remains unchanged.
// ignore_for_file: depend_on_referenced_packages, implementation_imports

import 'dart:async';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:flutter_local_notifications_linux/src/flutter_local_notifications.dart'
    as linux_impl;
import 'package:flutter_local_notifications_linux/src/notifications_manager.dart';
import 'package:flutter_local_notifications_linux/src/platform_info.dart';
import 'package:flutter_local_notifications_linux/src/storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/attention.dart';

class _PlatformInfo extends LinuxPlatformInfo {
  _PlatformInfo(this.runtimePath);

  final String runtimePath;

  @override
  Future<LinuxPlatformInfoData> getAll() async => LinuxPlatformInfoData(
    appName: 'OrgBots fixture',
    assetsPath: '/tmp/orgbots-fixture-assets',
    runtimePath: runtimePath,
  );
}

class _NotificationsService extends DBusObject {
  _NotificationsService(this.notifySeen)
    : super(DBusObjectPath('/org/freedesktop/Notifications'));

  final Completer<List<DBusValue>> notifySeen;

  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(
      'org.freedesktop.Notifications',
      methods: [DBusIntrospectMethod('Notify')],
    ),
  ];

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != 'org.freedesktop.Notifications' ||
        call.name != 'Notify') {
      return DBusMethodErrorResponse.unknownMethod();
    }
    notifySeen.complete(call.values);
    return DBusMethodSuccessResponse([const DBusUint32(73)]);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'Linux plugin Notify and synthetic default click route a room',
    () async {
      final temp = await Directory.systemTemp.createTemp('orgbots-notify-');
      final serviceClient = DBusClient.session();
      try {
        debugDefaultTargetPlatformOverride = TargetPlatform.linux;
        await Directory('${temp.path}/runtime').create();
        final notifySeen = Completer<List<DBusValue>>();
        final service = _NotificationsService(notifySeen);
        await serviceClient.requestName('org.freedesktop.Notifications');
        await serviceClient.registerObject(service);

        final platformInfo = _PlatformInfo('${temp.path}/runtime');
        final manager = LinuxNotificationManager.private(
          platformInfo: platformInfo,
          storage: NotificationStorage(platformInfo: platformInfo),
        );
        FlutterLocalNotificationsPlatform.instance =
            linux_impl.LinuxFlutterLocalNotificationsPlugin.private(manager);
        final attentions = Attentions();
        final routed = Completer<AttentionTarget>();
        attentions.onRouteRequested = routed.complete;

        await attentions.consider({
          'envelope': {'id': 'fixture-event-7'},
          'notification': {
            'reason': 'decision_needed',
            'body': 'Fixture question requires an owner answer',
          },
        }, roomId: 'fixture-room-3');

        final values = await notifySeen.future.timeout(
          const Duration(seconds: 5),
        );
        expect((values[0] as DBusString).value, 'OrgBots fixture');
        expect((values[3] as DBusString).value, 'A decision needs you');
        expect((values[4] as DBusString).value, contains('Fixture question'));
        final actions = (values[5] as DBusArray).children
            .map((value) => (value as DBusString).value)
            .toList();
        expect(actions, ['default', 'open']);

        await service.emitSignal(
          'org.freedesktop.Notifications',
          'ActionInvoked',
          [const DBusUint32(73), const DBusString('default')],
        );
        final target = await routed.future.timeout(const Duration(seconds: 5));
        expect(target.roomId, 'fixture-room-3');
        expect(target.reason, Attention.decision);
        expect(target.eventId, 'fixture-event-7');
      } finally {
        debugDefaultTargetPlatformOverride = null;
        await serviceClient.close();
        await temp.delete(recursive: true);
      }
    },
    skip:
        Platform.environment['ORGBOTS_PRIVATE_DBUS_TEST'] == '1' &&
            (Platform.environment['DBUS_SESSION_BUS_ADDRESS'] ?? '').contains(
              'unix:path=/tmp/dbus-',
            )
        ? false
        : 'Run only under dbus-run-session with ORGBOTS_PRIVATE_DBUS_TEST=1',
  );
}
