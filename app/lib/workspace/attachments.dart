import 'dart:convert';
import 'dart:io';
import 'package:flutter/services.dart';
import '../protocol/models.dart';
import '../state/room_archive.dart';

const attachmentChannel = MethodChannel('orgbot/attachments');

class RoomAttachments {
  RoomAttachments(this.roomId);
  final String roomId;
  static const maxFiles = 5;
  static const maxBytes = 20 * 1024 * 1024;
  Future<Directory> get directory async =>
      Directory('${(await RoomArchive.directory(roomId)).path}/attachments');

  Future<List<ChatFile>> pick() async {
    final picked =
        await attachmentChannel.invokeListMethod<Object?>('pick', {
          'room': roomId,
        }) ??
        [];
    final files = <ChatFile>[];
    for (final raw in picked) {
      var file = ChatFile.fromJson((raw as Map).cast<String, Object?>());
      try {
        final prepared = await attachmentChannel
            .invokeMapMethod<String, Object?>('prepare', {
              'room': roomId,
              'id': file.id,
            });
        file = ChatFile.fromJson(prepared!);
      } on PlatformException catch (e) {
        file = ChatFile(
          id: file.id,
          name: file.name,
          mimeType: file.mimeType,
          bytes: file.bytes,
          note:
              'Original saved. Could not extract contents: ${e.message ?? 'unsupported document'}.',
        );
        final manifest = '${(await directory).path}/${file.id}/manifest.json';
        final temporary = File('$manifest.tmp');
        await temporary.writeAsString(jsonEncode(file.toJson()), flush: true);
        await temporary.rename(manifest);
      }
      files.add(file);
    }
    return files;
  }

  Future<File> original(String id) async {
    if (!RegExp(r'^att_[a-f0-9]{32}$').hasMatch(id)) {
      throw const FormatException('Invalid attachment');
    }
    final root = await (await directory).resolveSymbolicLinks();
    final file = File('$root/$id/original');
    final path = await file.resolveSymbolicLinks();
    if (!path.startsWith('$root/')) {
      throw const FormatException('Attachment leaves this room');
    }
    return File(path);
  }

  Future<void> open(ChatFile file) => attachmentChannel.invokeMethod<void>(
    'open',
    {'room': roomId, 'id': file.id},
  );

  Future<void> saveDraft(List<ChatFile> files) async {
    final dir = await directory;
    await dir.create(recursive: true);
    final temp = File('${dir.path}/draft.tmp');
    await temp.writeAsString(
      jsonEncode(files.map((f) => f.toJson()).toList()),
      flush: true,
    );
    await temp.rename('${dir.path}/draft.json');
  }

  Future<List<ChatFile>> loadDraft() async {
    final file = File('${(await directory).path}/draft.json');
    if (!await file.exists()) return [];
    if (await file.length() > 64 * 1024) {
      throw const FormatException('Attachment draft is too large');
    }
    return [
      for (final raw in jsonDecode(await file.readAsString()) as List)
        ChatFile.fromJson((raw as Map).cast<String, Object?>()),
    ];
  }
}
