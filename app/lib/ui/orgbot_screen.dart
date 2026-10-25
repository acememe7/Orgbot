// ignore_for_file: prefer_interpolation_to_compose_strings

// OrgBot home: your bots, and rooms as group chats of those bots.
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'polish.dart';
import '../orgbot/credentials.dart';
import 'sign_in_screen.dart';
import 'connect_screen.dart';
import '../workspace/workspace.dart';
import '../workspace/workspace_screen.dart';

import '../engine/agent_cli.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../orgbot/model_catalog.dart';
import 'brand.dart';
import 'engine_room_screen.dart';
import 'room_history_screen.dart';
import 'role_icons.dart';
import 'desktop_integrations_view.dart';
import '../workspace/desktop_files.dart';
import '../orgbot/roles.dart';

class OrgBotScreen extends StatefulWidget {
  final Library library;
  final Credentials? credentials;
  final bool? desktopClis;
  const OrgBotScreen({
    super.key,
    required this.library,
    this.credentials,
    this.desktopClis,
  });

  @override
  State<OrgBotScreen> createState() => _OrgBotScreenState();
}

class _OrgBotScreenState extends State<OrgBotScreen>
    with SingleTickerProviderStateMixin {
  int _tab = 0;
  String? _selectedRoomId;
  final _pages = PageController();
  final _search = List.generate(2, (_) => TextEditingController());
  late final AnimationController _desktopPageFade;
  bool get _desktopClis => widget.desktopClis ?? canRunAgentClis;

  @override
  void initState() {
    super.initState();
    _desktopPageFade = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 110),
      value: 1,
    );
    widget.library.load();
  }

  @override
  void dispose() {
    _pages.dispose();
    _desktopPageFade.dispose();
    for (final search in _search) {
      search.dispose();
    }
    super.dispose();
  }

  void _selectTab(int index) {
    if (_tab == index) return;
    FocusManager.instance.primaryFocus?.unfocus();
    if (!_desktopClis) HapticFeedback.selectionClick();
    setState(() {
      _tab = index;
    });
    if (_desktopClis) {
      if (_pages.hasClients) _pages.jumpToPage(index);
      if (MediaQuery.disableAnimationsOf(context)) {
        _desktopPageFade.value = 1;
      } else {
        _desktopPageFade.forward(from: 0);
      }
    } else if (MediaQuery.disableAnimationsOf(context)) {
      _pages.jumpToPage(index);
    } else {
      _pages.animateToPage(
        index,
        duration: motionDuration(context, 360),
        curve: Curves.easeOutCubic,
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final credentials = widget.credentials ?? ModelCatalog.credentials;
    return ListenableBuilder(
      listenable: Listenable.merge([widget.library, ?credentials]),
      builder: (context, _) {
        final lib = widget.library;
        final scheme = Theme.of(context).colorScheme;
        if (_desktopClis) return _desktop(lib, credentials);
        return Scaffold(
          appBar: AppBar(
            toolbarHeight: 62,
            titleSpacing: 24,
            title: Text(
              'ORGBOT',
              style: TextStyle(
                fontSize: 12,
                letterSpacing: 2.2,
                fontWeight: FontWeight.w600,
                color: scheme.onSurfaceVariant,
              ),
            ),
            actions: [
              IconButton(
                tooltip: 'Your profile',
                onPressed: lib.loaded ? _profile : null,
                icon: CircleAvatar(
                  radius: 17,
                  backgroundColor: scheme.surfaceContainerHighest,
                  child: Text(
                    lib.userName.trim().isEmpty
                        ? 'Y'
                        : lib.userName.trim().characters.first.toUpperCase(),
                    style: TextStyle(
                      fontFamily: 'Inter',
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: scheme.onSurface,
                    ),
                  ),
                ),
              ),
              PopupMenuButton<String>(
                tooltip: 'More',
                icon: const Icon(Icons.more_horiz, size: 22),
                onSelected: (_) => Navigator.push(
                  context,
                  MaterialPageRoute(builder: (_) => const ConnectScreen()),
                ),
                itemBuilder: (_) => [
                  const PopupMenuItem(
                    value: 'hosted',
                    child: Text('Join a hosted room'),
                  ),
                ],
              ),
              const SizedBox(width: 12),
            ],
          ),
          bottomNavigationBar: WorkspaceNavigation(
            selected: _tab,
            onSelected: _selectTab,
            desktop: _desktopClis,
          ),
          body: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 900),
              child: Column(
                children: [
                  if (lib.error != null)
                    MaterialBanner(
                      content: Text(lib.error!),
                      actions: [
                        TextButton(
                          onPressed: lib.loaded ? lib.save : lib.load,
                          child: const Text('Retry'),
                        ),
                      ],
                    ),
                  Expanded(
                    child: FadeTransition(
                      opacity: _desktopClis
                          ? _desktopPageFade
                          : const AlwaysStoppedAnimation(1),
                      child: PageView(
                        controller: _pages,
                        physics: const NeverScrollableScrollPhysics(),
                        children: [
                          for (var i = 0; i < 3; i++)
                            RetainedPage(
                              child: TickerMode(
                                enabled: _tab == i,
                                child: Column(
                                  children: [
                                    _pageHeader(i, lib),
                                    Expanded(
                                      child: !lib.loaded
                                          ? const Center(
                                              child:
                                                  CircularProgressIndicator(),
                                            )
                                          : switch (i) {
                                              0 => _roomsList(lib),
                                              1 => _botsList(lib),
                                              _ =>
                                                _desktopClis
                                                    ? _CliSetup(
                                                        credentials:
                                                            credentials,
                                                      )
                                                    : credentials == null
                                                    ? const Center(
                                                        child: Text(
                                                          'Accounts are unavailable.',
                                                        ),
                                                      )
                                                    : SignInScreen(
                                                        credentials:
                                                            credentials,
                                                        embedded: true,
                                                      ),
                                            },
                                    ),
                                  ],
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _desktop(Library lib, Credentials? credentials) {
    final scheme = Theme.of(context).colorScheme;
    final compact = MediaQuery.sizeOf(context).width < 760;
    final selected = lib.rooms
        .where((r) => r.id == _selectedRoomId)
        .firstOrNull;
    return Scaffold(
      body: SafeArea(
        child: Row(
          children: [
            SizedBox(
              width: compact ? 64 : 224,
              child: compact
                  ? Material(
                      color: scheme.surfaceContainerLow,
                      child: Column(
                        children: [
                          const SizedBox(height: 20),
                          IconButton(
                            tooltip: 'Your profile',
                            onPressed: _profile,
                            icon: const BrandMark(size: 26),
                          ),
                          IconButton(
                            tooltip: 'New session',
                            onPressed: lib.loaded ? _newRoom : null,
                            icon: const Icon(Icons.add),
                          ),
                          Expanded(
                            child: ListView(
                              children: [
                                for (final room in lib.rooms)
                                  IconButton(
                                    tooltip: room.name,
                                    onPressed: () => _runRoom(room),
                                    icon: Icon(
                                      _selectedRoomId == room.id && _tab == 0
                                          ? Icons.chat_bubble
                                          : Icons.chat_bubble_outline,
                                    ),
                                  ),
                              ],
                            ),
                          ),
                          IconButton(
                            tooltip: 'Team',
                            onPressed: () => _selectTab(1),
                            icon: const Icon(Icons.people_outline),
                          ),
                          IconButton(
                            tooltip: 'Connections & tools',
                            onPressed: () => _selectTab(2),
                            icon: const Icon(Icons.tune),
                          ),
                          const SizedBox(height: 14),
                        ],
                      ),
                    )
                  : Material(
                      color: scheme.surfaceContainerLow,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          Padding(
                            padding: const EdgeInsets.fromLTRB(20, 24, 12, 24),
                            child: Row(
                              children: [
                                const BrandMark(size: 28),
                                const SizedBox(width: 10),
                                const Expanded(
                                  child: Text(
                                    'OrgBots',
                                    style: TextStyle(
                                      fontSize: 20,
                                      fontWeight: FontWeight.w600,
                                    ),
                                  ),
                                ),
                                IconButton(
                                  tooltip: 'Your profile',
                                  onPressed: lib.loaded ? _profile : null,
                                  icon: const Icon(
                                    Icons.account_circle_outlined,
                                    size: 20,
                                  ),
                                ),
                              ],
                            ),
                          ),
                          Padding(
                            padding: const EdgeInsets.symmetric(horizontal: 16),
                            child: OutlinedButton.icon(
                              onPressed: lib.loaded ? _newRoom : null,
                              icon: const Icon(Icons.add, size: 18),
                              label: const Text('New session'),
                            ),
                          ),
                          const SizedBox(height: 20),
                          Padding(
                            padding: const EdgeInsets.symmetric(horizontal: 20),
                            child: Text(
                              'Sessions',
                              style: TextStyle(
                                color: scheme.onSurfaceVariant,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ),
                          const SizedBox(height: 8),
                          Expanded(
                            child: !lib.loaded
                                ? const Center(
                                    child: CircularProgressIndicator(),
                                  )
                                : ListView.builder(
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 8,
                                    ),
                                    itemCount: lib.rooms.length,
                                    itemBuilder: (context, i) {
                                      final room = lib.rooms[i];
                                      final session = lib.sessions[room.id];
                                      return ListTile(
                                        dense: true,
                                        selected:
                                            _tab == 0 &&
                                            selected?.id == room.id,
                                        selectedTileColor: scheme
                                            .primaryContainer
                                            .withValues(alpha: .45),
                                        shape: RoundedRectangleBorder(
                                          borderRadius: BorderRadius.circular(
                                            8,
                                          ),
                                        ),
                                        title: Text(
                                          room.name,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                        ),
                                        subtitle: Text(
                                          session?.store?.runStateLabel ??
                                              'Saved',
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                        ),
                                        onTap: () => _runRoom(room),
                                        trailing: IconButton(
                                          tooltip: 'Session settings',
                                          icon: const Icon(
                                            Icons.more_horiz,
                                            size: 18,
                                          ),
                                          onPressed: () => _roomDialog(room),
                                        ),
                                      );
                                    },
                                  ),
                          ),
                          const Divider(height: 1),
                          for (final entry in [
                            (1, Icons.people_outline, 'Team'),
                            (2, Icons.tune, 'Connections & tools'),
                          ])
                            ListTile(
                              selected: _tab == entry.$1,
                              leading: Icon(entry.$2, size: 20),
                              title: Text(
                                entry.$3,
                                style: const TextStyle(fontSize: 13),
                              ),
                              onTap: () => _selectTab(entry.$1),
                            ),
                          const SizedBox(height: 10),
                        ],
                      ),
                    ),
            ),
            VerticalDivider(width: 1, color: scheme.outlineVariant),
            Expanded(
              child: Column(
                children: [
                  if (lib.error != null)
                    MaterialBanner(
                      content: Text(lib.error!),
                      actions: [
                        TextButton(
                          onPressed: lib.loaded ? lib.save : lib.load,
                          child: const Text('Retry'),
                        ),
                      ],
                    ),
                  Expanded(
                    child: _tab == 0 && selected != null
                        ? EngineRoomScreen(
                            key: ObjectKey(selected),
                            room: selected,
                            library: lib,
                            embedded: true,
                          )
                        : _tab == 0
                        ? Center(
                            child: ConstrainedBox(
                              constraints: const BoxConstraints(maxWidth: 480),
                              child: Padding(
                                padding: const EdgeInsets.all(32),
                                child: Column(
                                  mainAxisSize: MainAxisSize.min,
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      'A place to do the work.',
                                      style: Theme.of(
                                        context,
                                      ).textTheme.headlineLarge,
                                    ),
                                    const SizedBox(height: 16),
                                    Text(
                                      'Open a session to continue with your team, project files and saved context.',
                                      style: Theme.of(
                                        context,
                                      ).textTheme.bodyLarge,
                                    ),
                                    const SizedBox(height: 24),
                                    FilledButton.icon(
                                      onPressed: lib.loaded ? _newRoom : null,
                                      icon: const Icon(Icons.add),
                                      label: const Text('New session'),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          )
                        : Column(
                            children: [
                              _pageHeader(_tab, lib),
                              Expanded(
                                child: _tab == 1
                                    ? _botsList(lib)
                                    : _CliSetup(credentials: credentials),
                              ),
                            ],
                          ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _pageHeader(int page, Library lib) {
    final scheme = Theme.of(context).colorScheme;
    final title = page == 2
        ? (_desktopClis ? 'Setup' : 'Accounts')
        : ['Rooms', 'Team'][page];
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 10, 24, 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  style: Theme.of(context).textTheme.headlineLarge,
                ),
              ),
              if (page < 2)
                IconButton.filled(
                  tooltip: page == 0 ? 'New room' : 'New bot',
                  onPressed: !lib.loaded
                      ? null
                      : page == 0
                      ? _newRoom
                      : _newBot,
                  style: IconButton.styleFrom(minimumSize: const Size(46, 46)),
                  icon: const Icon(Icons.add_rounded, size: 24),
                ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            switch (page) {
              0 =>
                '${lib.rooms.length} ${lib.rooms.length == 1 ? 'room' : 'rooms'} · ${lib.bots.length} ${lib.bots.length == 1 ? 'teammate' : 'teammates'}',
              1 => '${lib.bots.length} teammates, each with a role.',
              2 when _desktopClis => 'Agent CLIs available to this workspace.',
              _ => 'Subscriptions and API access, saved on this device.',
            },
            style: TextStyle(
              fontSize: 13,
              height: 1.4,
              color: scheme.onSurfaceVariant,
            ),
          ),
          if (page < 2) ...[
            const SizedBox(height: 22),
            TextField(
              key: ValueKey('search-$page'),
              controller: _search[page],
              onChanged: (_) => setState(() {}),
              style: const TextStyle(fontSize: 14),
              decoration: InputDecoration(
                hintText: page == 0 ? 'Find a room' : 'Name, role or model',
                prefixIcon: Icon(
                  Icons.search_rounded,
                  size: 20,
                  color: scheme.onSurfaceVariant,
                ),
                suffixIcon: _search[page].text.isEmpty
                    ? null
                    : IconButton(
                        tooltip: 'Clear search',
                        icon: const Icon(Icons.close, size: 18),
                        onPressed: () => setState(_search[page].clear),
                      ),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 12,
                ),
                fillColor: scheme.surfaceContainerLow,
              ),
            ),
          ],
        ],
      ),
    );
  }

  // ---- bots ----
  Future<void> _profile() async {
    final name = TextEditingController(text: widget.library.userName);
    final profile = TextEditingController(text: widget.library.userProfile);
    final route = DialogRoute<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Your profile'),
        content: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: name,
                maxLength: 80,
                decoration: const InputDecoration(
                  labelText: 'What should the bots call you?',
                ),
              ),
              TextField(
                controller: profile,
                maxLength: 2000,
                minLines: 3,
                maxLines: 6,
                decoration: const InputDecoration(
                  labelText: 'About you and how you work',
                  helperText: 'Shared with the bots in rooms you start.',
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              if (name.text.trim().isEmpty) return;
              widget.library.userName = name.text.trim();
              widget.library.userProfile = profile.text.trim();
              await widget.library.save();
              if (context.mounted && widget.library.error == null) {
                Navigator.pop(context);
              }
            },
            child: const Text('Save'),
          ),
        ],
      ),
    );
    await Navigator.of(context).push(route);
    await route.completed;
    name.dispose();
    profile.dispose();
  }

  Widget _botsList(Library lib) {
    if (lib.bots.isEmpty) {
      return EmptyState(
        icon: Icons.people_outline,
        title: 'Build your team',
        message: 'Give a bot a name, choose its model and define its role.',
        actionLabel: 'Create a bot',
        onAction: _newBot,
      );
    }
    final credentials = widget.credentials ?? ModelCatalog.credentials;
    final query = _search[1].text.trim().toLowerCase();
    final bots = lib.bots
        .where(
          (b) => '${b.name} ${b.handle} ${b.roleDef.label} ${b.model}'
              .toLowerCase()
              .contains(query),
        )
        .toList();
    if (bots.isEmpty) {
      return const EmptyState(
        icon: Icons.search,
        title: 'No matching teammates',
        message: 'Try a name, role or model.',
      );
    }
    final scheme = Theme.of(context).colorScheme;
    final compactDesktop =
        _desktopClis && MediaQuery.sizeOf(context).width >= 760;
    return ListView.separated(
      key: const PageStorageKey('team-list'),
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      itemCount: bots.length,
      separatorBuilder: (_, _) => compactDesktop
          ? Divider(height: 1, color: scheme.outlineVariant)
          : const SizedBox(height: 10),
      itemBuilder: (context, i) {
        final b = bots[i];
        final handle = b.handle.isEmpty ? b.name : b.handle;
        final needsKey =
            b.needsKey && credentials?.isSignedIn(b.provider) != true;
        if (compactDesktop) {
          return _desktopBotRow(
            b,
            handle: handle,
            needsKey: needsKey,
            scheme: scheme,
            library: lib,
          );
        }
        return PressSurface(
          key: ValueKey(b.id),
          padding: const EdgeInsets.all(16),
          onTap: () => _botDialog(b),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              BotAvatar(handle: handle, radius: 23),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      b.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: 17,
                        fontWeight: FontWeight.w600,
                        letterSpacing: -0.4,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '@$handle · ${b.roles.map((key) => Roles.byKey(key).label).join(', ')}',
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 12,
                        height: 1.5,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: 12),
                    Text(
                      needsKey
                          ? 'Add a ${b.provider.label} key in ${_desktopClis ? 'Setup' : 'Accounts'}'
                          : ModelCatalog.labelFor(b.provider, b.model),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 11,
                        color: needsKey ? scheme.error : scheme.secondary,
                      ),
                    ),
                  ],
                ),
              ),
              SizedBox(
                width: 44,
                child: PopupMenuButton<String>(
                  tooltip: 'Bot options',
                  padding: EdgeInsets.zero,
                  icon: Icon(
                    Icons.more_horiz,
                    size: 20,
                    color: scheme.onSurfaceVariant,
                  ),
                  onSelected: (value) {
                    if (value == 'edit') {
                      _botDialog(b);
                    } else {
                      lib.removeBot(b.id);
                    }
                  },
                  itemBuilder: (_) => [
                    const PopupMenuItem(value: 'edit', child: Text('Edit bot')),
                    const PopupMenuItem(
                      value: 'delete',
                      child: Text('Remove bot'),
                    ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _desktopBotRow(
    BotDef bot, {
    required String handle,
    required bool needsKey,
    required ColorScheme scheme,
    required Library library,
  }) => Semantics(
    label:
        '${bot.name}, ${bot.roleDef.label}, ${ModelCatalog.labelFor(bot.provider, bot.model)}',
    button: true,
    child: Material(
      color: Colors.transparent,
      child: InkWell(
        key: ValueKey('desktop-bot-${bot.id}'),
        onTap: () => _botDialog(bot),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
          child: Row(
            children: [
              Expanded(
                flex: 3,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      bot.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '@$handle · ${bot.roles.map((key) => Roles.byKey(key).label).join(', ')}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 13,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 20),
              Expanded(
                flex: 2,
                child: Text(
                  needsKey
                      ? '${bot.provider.label} · API key needed'
                      : ModelCatalog.labelFor(bot.provider, bot.model),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 13,
                    color: needsKey ? scheme.error : scheme.secondary,
                  ),
                ),
              ),
              PopupMenuButton<String>(
                tooltip: 'Bot options',
                padding: EdgeInsets.zero,
                icon: Icon(
                  Icons.more_horiz,
                  size: 20,
                  color: scheme.onSurfaceVariant,
                ),
                onSelected: (value) {
                  if (value == 'edit') {
                    _botDialog(bot);
                  } else {
                    library.removeBot(bot.id);
                  }
                },
                itemBuilder: (_) => [
                  const PopupMenuItem(value: 'edit', child: Text('Edit bot')),
                  const PopupMenuItem(
                    value: 'delete',
                    child: Text('Remove bot'),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    ),
  );

  void _newBot() => _botDialog(null);

  Future<void> _botDialog(BotDef? existing) async {
    await showEditor<void>(
      context: context,
      builder: (context) => _BotEditor(
        existing: existing,
        desktopClis: _desktopClis,
        onSave: (bot) async {
          if (existing == null) {
            widget.library.addBot(bot);
          } else {
            widget.library.updateBot(bot);
          }
          await widget.library.save();
          if (widget.library.error != null) {
            throw StateError(widget.library.error!);
          }
        },
      ),
      durationMilliseconds: _desktopClis ? 140 : null,
    );
  }

  // ---- rooms ----
  Widget _roomsList(Library lib) {
    if (lib.rooms.isEmpty) {
      return EmptyState(
        icon: Icons.forum_outlined,
        title: 'A place for your next idea',
        message:
            'Bring your team, choose a workspace, and start a conversation.',
        actionLabel: 'Create a room',
        onAction: _newRoom,
      );
    }
    final query = _search[0].text.trim().toLowerCase();
    final rooms = lib.rooms
        .where(
          (r) => '${r.name} ${r.goal} ${r.workspace.path}'
              .toLowerCase()
              .contains(query),
        )
        .toList();
    if (rooms.isEmpty) {
      return const EmptyState(
        icon: Icons.search,
        title: 'No matching rooms',
        message: 'Try another name or workspace.',
      );
    }
    final scheme = Theme.of(context).colorScheme;
    return ListView.separated(
      key: const PageStorageKey('room-list'),
      itemCount: rooms.length,
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      separatorBuilder: (_, _) => const SizedBox(height: 14),
      itemBuilder: (context, i) {
        final room = rooms[i];
        final session = lib.sessions[room.id];
        final active =
            session?.isRunning == true && session?.store?.isActive == true;
        final state = active
            ? 'Running'
            : session?.store?.runStateLabel ?? 'Saved';
        final messages = session?.store?.messages;
        final preview = messages?.isNotEmpty == true
            ? messages!.last.body.replaceAll('\n', ' ')
            : room.goal;
        final folder = canRunAgentClis ? room.path : room.workspace.path;
        final members = [
          for (final slot in room.bots)
            ...lib.bots.where((b) => b.id == slot.botId),
        ].take(4).toList();
        final menu = PopupMenuButton<String>(
          tooltip: 'Room options',
          onSelected: (action) async {
            if (action == 'edit') _roomDialog(room);
            if (action == 'stop') await lib.stopRoom(room.id);
            if (action == 'history' && context.mounted) {
              Navigator.push(
                context,
                MaterialPageRoute(
                  builder: (_) =>
                      RoomHistoryScreen(roomId: room.id, roomName: room.name),
                ),
              );
            }
            if (action == 'files' && context.mounted) {
              Navigator.push(
                context,
                MaterialPageRoute(
                  builder: (_) => WorkspaceFilesScreen(
                    config: room.workspace,
                    roomId: room.id,
                  ),
                ),
              );
            }
            if (action == 'delete') lib.removeRoom(room.id);
          },
          itemBuilder: (_) => [
            const PopupMenuItem(value: 'edit', child: Text('Room settings')),
            if (session != null)
              const PopupMenuItem(value: 'stop', child: Text('Stop and save')),
            if (!canRunAgentClis)
              const PopupMenuItem(
                value: 'files',
                child: Text('Working folder'),
              ),
            const PopupMenuItem(value: 'history', child: Text('Saved history')),
            const PopupMenuItem(value: 'delete', child: Text('Remove room')),
          ],
        );
        return PressSurface(
          key: ValueKey(room.id),
          onTap: () => _runRoom(room),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(
                    room.workspace.isRemote
                        ? Icons.terminal_rounded
                        : Icons.chat_bubble_outline_rounded,
                    size: 18,
                    color: scheme.tertiary,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      room.workspace.isRemote
                          ? 'REMOTE WORKSPACE'
                          : canRunAgentClis
                          ? 'THIS COMPUTER'
                          : 'PHONE WORKSPACE',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 9,
                        letterSpacing: 1.1,
                        fontWeight: FontWeight.w500,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                  if (active)
                    Padding(
                      padding: const EdgeInsets.only(right: 5),
                      child: Icon(
                        Icons.circle,
                        size: 5,
                        color: scheme.tertiary,
                      ),
                    ),
                  Text(
                    state,
                    style: TextStyle(
                      fontSize: 10,
                      color: active ? scheme.tertiary : scheme.onSurfaceVariant,
                    ),
                  ),
                  SizedBox(width: 44, height: 44, child: menu),
                ],
              ),
              const SizedBox(height: 4),
              Text(
                room.name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 22,
                  height: 1.25,
                  fontWeight: FontWeight.w600,
                  letterSpacing: -0.7,
                ),
              ),
              if (preview.isNotEmpty) ...[
                const SizedBox(height: 10),
                Text(
                  preview,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 13,
                    height: 1.6,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ],
              const SizedBox(height: 20),
              Divider(color: scheme.outlineVariant.withValues(alpha: 0.5)),
              const SizedBox(height: 16),
              Row(
                children: [
                  if (members.isNotEmpty) ...[
                    SizedBox(
                      width: 26 + (members.length - 1) * 16,
                      height: 26,
                      child: Stack(
                        children: [
                          for (var i = 0; i < members.length; i++)
                            Positioned(
                              left: i * 16,
                              top: 0,
                              child: DecoratedBox(
                                decoration: BoxDecoration(
                                  shape: BoxShape.circle,
                                  border: Border.all(
                                    color: scheme.surfaceContainerLow,
                                    width: 2,
                                  ),
                                ),
                                child: BotAvatar(
                                  handle: members[i].handle,
                                  radius: 11,
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 8),
                  ],
                  Text(
                    '${room.bots.length}',
                    style: TextStyle(
                      fontSize: 11,
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(width: 16),
                  Icon(
                    Icons.folder_outlined,
                    size: 14,
                    color: scheme.onSurfaceVariant,
                  ),
                  const SizedBox(width: 5),
                  Expanded(
                    child: Text(
                      folder.isEmpty
                          ? (canRunAgentClis ? 'This computer' : 'This phone')
                          : folder,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      textAlign: TextAlign.right,
                      style: TextStyle(
                        fontSize: 11,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                ],
              ),
            ],
          ),
        );
      },
    );
  }

  void _newRoom() => _roomDialog();

  Future<void> _roomDialog([RoomDef? existing]) async {
    final id = existing?.id ?? genId('room');
    final name = TextEditingController(text: existing?.name);
    final goal = TextEditingController(text: existing?.goal ?? '');
    final path = TextEditingController(text: existing?.path);
    var workspace = existing?.workspace.copy() ?? WorkspaceConfig();
    final desktopRoom = _desktopClis;
    final defaultTurns = desktopRoom ? desktopDefaultMaxTurns : defaultMaxTurns;
    final defaultParallel = desktopRoom
        ? desktopDefaultParallelTurns
        : defaultParallelTurns;
    var maxTurns = existing?.maxTurns ?? defaultTurns;
    var parallelTurns = existing?.parallelTurns ?? defaultParallel;
    final turnChoices = desktopRoom
        ? desktopTurnBudgetChoices
        : turnBudgetChoices;
    final availableTurnChoices = {
      if (!turnChoices.contains(maxTurns)) maxTurns,
      ...turnChoices,
    }.toList();
    final availableParallelChoices = {
      if (!parallelTurnChoices.contains(parallelTurns)) parallelTurns,
      ...parallelTurnChoices,
    }.toList();
    final selected = <String, String>{
      for (final slot in existing?.bots ?? <RoomSlot>[]) slot.botId: '',
    }; // botId -> role

    final route = DialogRoute<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setLocal) => _editor(
          context,
          title: Text(existing == null ? 'New room' : 'Edit room'),
          content: SizedBox(
            width: 460,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  FormSection(
                    title: 'Brief',
                    children: [
                      TextField(
                        controller: name,
                        decoration: const InputDecoration(
                          labelText: 'Room name',
                        ),
                      ),
                      const SizedBox(height: 12),
                      TextField(
                        controller: goal,
                        maxLines: 3,
                        decoration: const InputDecoration(labelText: 'Goal'),
                      ),
                      const SizedBox(height: 12),
                      // A spend control, not a deadline. It was fixed at 500,
                      // which a busy room spends in about a day — and then it
                      // stopped and waited, sometimes for hours, for someone
                      // to notice.
                      DropdownButtonFormField<int>(
                        initialValue: maxTurns,
                        decoration: const InputDecoration(
                          labelText: 'Turn budget',
                          helperText:
                              'Provider allowance is used while agents work; '
                              'viewing an idle room does not. This pauses on '
                              'turns, not an exact token cap.',
                        ),
                        items: [
                          for (final choice in availableTurnChoices)
                            DropdownMenuItem(
                              value: choice,
                              child: Text(
                                choice == 0
                                    ? 'No limit'
                                    : '$choice turns${turnChoices.contains(choice) ? '' : ' · saved'}',
                              ),
                            ),
                        ],
                        onChanged: (value) =>
                            setLocal(() => maxTurns = value ?? defaultTurns),
                      ),
                      const SizedBox(height: 12),
                      // The room used to give one bot a turn at a time, so
                      // most of the roster sat idle no matter how many bots
                      // were in it, and seating more could not make the work
                      // finish sooner.
                      DropdownButtonFormField<int>(
                        initialValue: parallelTurns,
                        decoration: const InputDecoration(
                          labelText: 'Bots working at once',
                          helperText:
                              'Most of a turn is waiting on a model, so this '
                              'is what decides how much of the room is busy — '
                              'and it multiplies how fast you draw on your '
                              'plan. Four at once spends roughly four times '
                              'as much per hour as one.',
                        ),
                        items: [
                          for (final choice in availableParallelChoices)
                            DropdownMenuItem(
                              value: choice,
                              child: Text(
                                '${choice == 1 ? 'One at a time' : '$choice bots'}${parallelTurnChoices.contains(choice) ? '' : ' · saved'}',
                              ),
                            ),
                        ],
                        onChanged: (value) => setLocal(
                          () => parallelTurns = value ?? defaultParallel,
                        ),
                      ),
                      const SizedBox(height: 12),
                    ],
                  ),
                  FormSection(
                    title: 'Workspace',
                    children: [
                      if (canRunAgentClis)
                        TextField(
                          controller: path,
                          decoration: InputDecoration(
                            labelText: 'Project path (system CLIs)',
                            suffixIcon: IconButton(
                              tooltip: 'Choose project folder',
                              icon: const Icon(Icons.folder_open),
                              onPressed: () async {
                                try {
                                  final folder =
                                      await DesktopFiles.chooseProjectFolder();
                                  if (folder != null && context.mounted) {
                                    path.text = folder;
                                  }
                                } catch (error) {
                                  if (context.mounted) {
                                    ScaffoldMessenger.of(context).showSnackBar(
                                      SnackBar(
                                        content: Text(
                                          'Could not choose folder: $error',
                                        ),
                                      ),
                                    );
                                  }
                                }
                              },
                            ),
                          ),
                        )
                      else
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          leading: Icon(
                            workspace.isRemote
                                ? Icons.dns_outlined
                                : Icons.phone_android,
                          ),
                          title: Text(workspace.label),
                          subtitle: Text(
                            workspace.isRemote
                                ? workspace.path
                                : 'Phone workspace',
                          ),
                          trailing: const Icon(Icons.chevron_right),
                          onTap: () async {
                            final selected =
                                await Navigator.push<WorkspaceConfig>(
                                  context,
                                  MaterialPageRoute(
                                    builder: (_) => WorkspaceScreen(
                                      config: workspace,
                                      roomId: id,
                                      onSave: existing == null
                                          ? null
                                          : (config) async {
                                              await widget.library.stopRoom(
                                                existing.id,
                                              );
                                              existing.workspace = config
                                                  .copy();
                                              await widget.library.save();
                                              if (widget.library.error !=
                                                  null) {
                                                throw StateError(
                                                  widget.library.error!,
                                                );
                                              }
                                            },
                                    ),
                                  ),
                                );
                            if (selected != null && context.mounted) {
                              setLocal(() => workspace = selected);
                            }
                          },
                        ),
                    ],
                  ),
                  FormSection(
                    title: 'Team',
                    children: [
                      // Each bot brings its own role, set where the bot is
                      // defined — the same reviewer reviews in every room.
                      for (final b in widget.library.bots)
                        CheckboxListTile(
                          dense: true,
                          value: selected.containsKey(b.id),
                          secondary: BotAvatar(
                            handle: b.handle.isEmpty ? b.name : b.handle,
                            radius: 18,
                          ),
                          title: Text(b.name),
                          subtitle: Text(
                            b.roles
                                    .map((role) => Roles.byKey(role).label)
                                    .join(', ') +
                                ' · ' +
                                b.provider.label,
                          ),
                          onChanged: (v) => setLocal(() {
                            if (v == true) {
                              selected[b.id] = b.role;
                            } else {
                              selected.remove(b.id);
                            }
                          }),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () async {
                if (name.text.trim().isEmpty) {
                  ScaffoldMessenger.of(context).showSnackBar(
                    const SnackBar(content: Text('Give the room a name')),
                  );
                  return;
                }
                if (selected.isEmpty) {
                  ScaffoldMessenger.of(context).showSnackBar(
                    const SnackBar(content: Text('Choose at least one bot')),
                  );
                  return;
                }
                final room = RoomDef(
                  id: id,
                  workspace: workspace,
                  memory: existing?.memory ?? '',
                  name: name.text.trim(),
                  goal: goal.text.trim(),
                  path: path.text.trim(),
                  maxTurns: maxTurns,
                  parallelTurns: parallelTurns,
                  tasks: [goal.text.trim()].where((t) => t.isNotEmpty).toList(),
                  bots: selected.keys.map(RoomSlot.new).toList(),
                );
                if (existing == null) {
                  widget.library.addRoom(room);
                } else {
                  await widget.library.stopRoom(existing.id);
                  final index = widget.library.rooms.indexWhere(
                    (r) => r.id == existing.id,
                  );
                  if (index >= 0) widget.library.rooms[index] = room;
                  widget.library.save();
                }
                await widget.library.save();
                if (context.mounted && widget.library.error == null) {
                  Navigator.pop(context);
                }
              },
              child: Text(existing == null ? 'Create' : 'Save'),
            ),
          ],
        ),
      ),
    );
    await Navigator.push(context, route);
    await route.completed;
    name.dispose();
    goal.dispose();
    path.dispose();
  }

  void _runRoom(RoomDef room) {
    if (_desktopClis) {
      setState(() {
        _selectedRoomId = room.id;
        _tab = 0;
      });
      return;
    }
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => EngineRoomScreen(
          room: room,
          library: widget.library,
          desktopLayout: _desktopClis,
        ),
      ),
    );
  }
}

class _CliSetup extends StatefulWidget {
  const _CliSetup({required this.credentials});

  final Credentials? credentials;

  @override
  State<_CliSetup> createState() => _CliSetupState();
}

class _CliSetupState extends State<_CliSetup> {
  final TextEditingController _apiKey = TextEditingController();

  @override
  void initState() {
    super.initState();
    widget.credentials?.load();
  }

  @override
  void dispose() {
    _apiKey.dispose();
    super.dispose();
  }

  Future<void> _editApiKey(ProviderKind provider) async {
    final credentials = widget.credentials;
    if (credentials == null) return;
    _apiKey.clear();
    final formKey = GlobalKey<FormState>();
    final accepted = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('${provider.label} API key'),
        content: Form(
          key: formKey,
          child: TextFormField(
            controller: _apiKey,
            autofocus: true,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            decoration: const InputDecoration(labelText: 'API key'),
            validator: (value) => value == null || value.trim().isEmpty
                ? 'Enter an API key.'
                : null,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () {
              if (formKey.currentState!.validate()) {
                Navigator.pop(context, true);
              }
            },
            child: const Text('Save key'),
          ),
        ],
      ),
    );
    if (accepted == true) {
      try {
        await credentials.saveApiKey(provider, _apiKey.text);
      } catch (_) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Could not save that API key.')),
          );
        }
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.credentials ?? Listenable.merge([]),
    builder: (context, _) => ListView(
      key: const ValueKey('cli-setup'),
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      children: [
        for (final cli in CliKind.values)
          _CliStatusTile(key: ValueKey('cli-${cli.name}'), cli: cli),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            onPressed: () => setState(() {}),
            icon: const Icon(Icons.refresh),
            label: const Text('Refresh CLI status'),
          ),
        ),
        const SizedBox(height: 14),
        Text('API access', style: Theme.of(context).textTheme.titleLarge),
        const SizedBox(height: 8),
        for (final provider in [ProviderKind.deepseek, ProviderKind.ollama])
          _ApiKeySetupTile(
            provider: provider,
            saved:
                widget.credentials?.forProvider(provider)?.kind ==
                CredentialKind.apiKey,
            onEdit: () => _editApiKey(provider),
            onRemove: () => widget.credentials?.forget(provider),
          ),
        const DesktopIntegrationsView(),
      ],
    ),
  );
}

class _ApiKeySetupTile extends StatelessWidget {
  const _ApiKeySetupTile({
    required this.provider,
    required this.saved,
    required this.onEdit,
    required this.onRemove,
  });

  final ProviderKind provider;
  final bool saved;
  final VoidCallback onEdit;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Material(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      borderRadius: BorderRadius.circular(16),
      child: ListTile(
        title: Text('${provider.label} API'),
        subtitle: Text(saved ? 'API key saved securely' : 'No API key saved'),
        trailing: Wrap(
          spacing: 4,
          children: [
            TextButton(
              onPressed: onEdit,
              child: Text(saved ? 'Change' : 'Add'),
            ),
            if (saved)
              IconButton(
                tooltip: 'Remove ${provider.label} API key',
                onPressed: onRemove,
                icon: const Icon(Icons.delete_outline),
              ),
          ],
        ),
      ),
    ),
  );
}

class _CliStatusTile extends StatelessWidget {
  const _CliStatusTile({super.key, required this.cli});
  final CliKind cli;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final status = findCli(cli);
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Material(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(20),
        child: ListTile(
          title: Text(_cliLabel(cli)),
          subtitle: Text(status.path ?? status.problem ?? 'Unavailable'),
          trailing: Text(status.available ? 'Installed' : 'Missing'),
        ),
      ),
    );
  }
}

String _cliLabel(CliKind cli) => switch (cli) {
  CliKind.codex => 'Codex CLI',
  CliKind.claude => 'Claude Code',
  CliKind.grok => 'Grok CLI',
};

/// The bot editor.
///
/// Split out of the screen because it has real state of its own: the model
/// list is fetched from the provider (a CLI call or an HTTP round trip), so it
/// arrives after the dialog is already on screen.
class _BotEditor extends StatefulWidget {
  final BotDef? existing;
  final bool desktopClis;
  final Future<void> Function(BotDef) onSave;
  const _BotEditor({
    required this.existing,
    required this.onSave,
    required this.desktopClis,
  });

  @override
  State<_BotEditor> createState() => _BotEditorState();
}

class _BotEditorState extends State<_BotEditor> {
  late final TextEditingController _name;
  late final TextEditingController _handle;
  late final TextEditingController _persona;
  late final TextEditingController _key;
  final TextEditingController _customModel = TextEditingController();

  late ProviderKind _provider;
  late String _role;
  late Set<String> _additionalRoles;
  RolePowers? _powers;
  String _model = '';
  bool _custom = false;
  bool _saving = false;
  bool _replaceKey = false;

  ModelList? _models;
  bool _loadingModels = false;

  /// Sentinel for the "type your own id" row.
  static const String _customValue = '__custom__';

  @override
  void initState() {
    super.initState();
    final b = widget.existing;
    _name = TextEditingController(text: b?.name ?? '');
    _handle = TextEditingController(text: b?.handle ?? '');
    _persona = TextEditingController(text: b?.persona ?? '');
    _key = TextEditingController(text: b?.apiKey ?? '');
    _provider = b?.provider ?? ProviderKind.codex;
    _role = b?.role ?? Roles.defaultKey;
    _additionalRoles = {...?b?.additionalRoles};
    _powers = b?.powers;
    _model = b?.model ?? '';
    _loadModels();
  }

  @override
  void dispose() {
    _name.dispose();
    _handle.dispose();
    _persona.dispose();
    _key.dispose();
    _customModel.dispose();
    super.dispose();
  }

  Future<void> _loadModels({bool refresh = false}) async {
    final provider = _provider;
    if (widget.desktopClis && !_isSupportedDesktopProvider) return;
    setState(() {
      _loadingModels = true;
      _models = null;
    });
    final list = await ModelCatalog.load(
      provider,
      apiKey: _key.text.trim(),
      refresh: refresh,
      canRunClis: widget.desktopClis,
    );
    if (!mounted || provider != _provider) return;
    setState(() {
      _models = list;
      _loadingModels = false;
      if (!_custom &&
          _model.isEmpty &&
          list.options.isNotEmpty &&
          !list.options.any((o) => o.id.isEmpty)) {
        _model = list.options.first.id;
      }
      // An id that the provider does not offer is a custom one; keep it and
      // open on Custom rather than silently switching the bot's model.
      if (_model.isNotEmpty && !list.options.any((o) => o.id == _model)) {
        _custom = true;
        _customModel.text = _model;
      }
    });
  }

  bool get _isSupportedDesktopProvider =>
      _provider == ProviderKind.anthropic ||
      _provider == ProviderKind.codex ||
      _provider == ProviderKind.grok ||
      _provider == ProviderKind.deepseek ||
      _provider == ProviderKind.ollama;

  void _changeProvider(ProviderKind next) {
    setState(() {
      _provider = next;
      // Credentials belong to one provider; never send a previous key elsewhere.
      _key.clear();
      _replaceKey = false;
      // Model ids do not carry across providers.
      _model = ModelCatalog.defaultFor(next);
      _custom = false;
      _customModel.clear();
    });
    _loadModels();
  }

  String get _resolvedModel => _custom ? _customModel.text.trim() : _model;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return _editor(
      context,
      title: Text(widget.existing == null ? 'New bot' : 'Edit bot'),
      content: SizedBox(
        width: 430,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              ListenableBuilder(
                listenable: Listenable.merge([_name, _handle]),
                builder: (context, _) => Padding(
                  padding: const EdgeInsets.fromLTRB(4, 8, 4, 28),
                  child: Row(
                    children: [
                      BotAvatar(
                        handle: _handle.text.isEmpty
                            ? _name.text
                            : _handle.text,
                        radius: 28,
                      ),
                      const SizedBox(width: 16),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              _name.text.trim().isEmpty
                                  ? 'Your next teammate'
                                  : _name.text,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: Theme.of(context).textTheme.titleLarge,
                            ),
                            const SizedBox(height: 5),
                            Text(
                              'Set their role, model and instructions.',
                              style: TextStyle(
                                fontSize: 12,
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              FormSection(
                title: 'Identity',
                children: [
                  TextField(
                    controller: _name,
                    textCapitalization: TextCapitalization.words,
                    decoration: const InputDecoration(
                      labelText: 'Name',
                      hintText: 'Iris',
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _handle,
                    decoration: const InputDecoration(
                      labelText: 'Handle',
                      prefixText: '@',
                      hintText: 'iris',
                    ),
                  ),
                ],
              ),
              FormSection(
                title: 'Role & permissions',
                children: [_rolePicker(scheme)],
              ),
              FormSection(
                title: widget.desktopClis
                    ? 'Model & access'
                    : 'Model & account',
                children: [
                  DropdownButtonFormField<ProviderKind>(
                    initialValue: _provider,
                    isExpanded: true,
                    decoration: const InputDecoration(labelText: 'Provider'),
                    items:
                        (widget.desktopClis
                                ? [
                                    ProviderKind.codex,
                                    ProviderKind.anthropic,
                                    ProviderKind.grok,
                                    ProviderKind.deepseek,
                                    ProviderKind.ollama,
                                    if (!_isSupportedDesktopProvider) _provider,
                                  ]
                                : ProviderKind.values
                                  // Offer what could actually run here. A provider whose
                                  // only sign-in is a CLI cannot be set up on a phone —
                                  // but it can already be signed in, because `orgbot
                                  // pair` carries a desktop sign-in across, and the
                                  // engine then calls the provider's API directly. That
                                  // is how Claude gets onto a phone; hiding it whenever
                                  // there was no CLI hid an account that was already
                                  // there and working.
                                  .where(
                                    (k) =>
                                        k.cli == null ||
                                        widget.desktopClis ||
                                        k.signsInOnDevice ||
                                        ModelCatalog.credentials?.isSignedIn(
                                              k,
                                            ) ==
                                            true,
                                  ))
                            .map(
                              (k) => DropdownMenuItem(
                                value: k,
                                child: Text(
                                  widget.desktopClis &&
                                          !_isSupportedDesktopProviderFor(k)
                                      ? '${k.label} (unavailable here)'
                                      : widget.desktopClis && k.cli != null
                                      ? _cliLabel(k.cli!)
                                      : widget.desktopClis && k.needsApiKey
                                      ? '${k.label} API'
                                      : k.label,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                            )
                            .toList(),
                    onChanged: (v) => _changeProvider(v ?? _provider),
                  ),
                  if (widget.desktopClis && !_isSupportedDesktopProvider)
                    const Padding(
                      padding: EdgeInsets.only(top: 8),
                      child: Text(
                        'Choose a CLI above to use this bot in Linux.',
                      ),
                    ),
                  const SizedBox(height: 10),
                  _authRow(scheme),
                  if (_provider != ProviderKind.scripted) ...[
                    const SizedBox(height: 10),
                    _modelField(scheme),
                  ],
                ],
              ),
              FormSection(
                title: 'Instructions',
                children: [
                  TextField(
                    controller: _persona,
                    minLines: 3,
                    maxLines: 6,
                    decoration: const InputDecoration(
                      hintText: 'How should this teammate approach their work?',
                      border: InputBorder.none,
                      enabledBorder: InputBorder.none,
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _saving ? null : _save,
          child: Text(_saving ? 'Saving…' : 'Save'),
        ),
      ],
    );
  }

  bool _isSupportedDesktopProviderFor(ProviderKind kind) =>
      kind == ProviderKind.anthropic ||
      kind == ProviderKind.codex ||
      kind == ProviderKind.grok ||
      kind == ProviderKind.deepseek ||
      kind == ProviderKind.ollama;

  /// The job the bot holds, grouped the way an organisation is.
  Widget _rolePicker(ColorScheme scheme) {
    final role = Roles.byKey(_role);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropdownButtonFormField<String>(
          initialValue: _role,
          isExpanded: true,
          decoration: const InputDecoration(labelText: 'Role'),
          items: [
            for (final group in RoleGroup.values) ...[
              DropdownMenuItem<String>(
                enabled: false,
                value: '__group_${group.name}',
                child: Text(
                  group.label.toUpperCase(),
                  style: TextStyle(
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.6,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ),
              for (final r in Roles.inGroup(group))
                DropdownMenuItem<String>(
                  value: r.key,
                  child: Row(
                    children: [
                      Icon(roleIcon(r.icon), size: 16),
                      const SizedBox(width: 8),
                      Flexible(
                        child: Text(r.label, overflow: TextOverflow.ellipsis),
                      ),
                    ],
                  ),
                ),
            ],
          ],
          onChanged: (v) {
            if (v == null || v.startsWith('__group_')) return;
            // Changing the job resets any hand-tuned permissions, so the new
            // role's defaults are what actually apply.
            setState(() {
              _role = v;
              _powers = null;
            });
          },
        ),
        const SizedBox(height: 6),
        Text(
          role.summary,
          style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
        ),
        const SizedBox(height: 8),
        _powerChips(scheme, role),
        ExpansionTile(
          tilePadding: EdgeInsets.zero,
          title: const Text('Additional roles', style: TextStyle(fontSize: 13)),
          children: [
            Wrap(
              spacing: 6,
              children: [
                for (final extra in Roles.all.where((r) => r.key != _role))
                  FilterChip(
                    label: Text(extra.label),
                    selected: _additionalRoles.contains(extra.key),
                    onSelected: (value) => setState(() {
                      if (value) {
                        _additionalRoles.add(extra.key);
                      } else {
                        _additionalRoles.remove(extra.key);
                      }
                    }),
                  ),
              ],
            ),
          ],
        ),
      ],
    );
  }

  /// What the role is allowed to do, and a way to override it.
  Widget _powerChips(ColorScheme scheme, RoleDef role) {
    final powers = _powers ?? role.powers;
    Widget chip(
      String label,
      bool on,
      RolePowers Function(bool) apply, {
      bool enabled = true,
    }) => FilterChip(
      label: Text(label, style: const TextStyle(fontSize: 11)),
      selected: on,
      visualDensity: VisualDensity.compact,
      onSelected: enabled ? (v) => setState(() => _powers = apply(v)) : null,
    );
    return Wrap(
      spacing: 6,
      runSpacing: 4,
      children: [
        chip(
          'Creates tasks',
          powers.mayCreateTasks,
          (v) => powers.copyWith(mayCreateTasks: v),
          enabled: !powers.reviewOnly,
        ),
        chip(
          'Inspects work',
          powers.mayInspect,
          (v) => powers.copyWith(mayInspect: v),
        ),
        chip(
          'Delivers',
          powers.maySubmit,
          (v) => powers.copyWith(maySubmit: v),
          enabled: !powers.reviewOnly,
        ),
        chip(
          'Review only',
          powers.reviewOnly,
          (v) => v
              ? powers.copyWith(
                  reviewOnly: true,
                  mayInspect: true,
                  mayCreateTasks: false,
                  maySubmit: false,
                )
              : powers.copyWith(reviewOnly: false),
        ),
      ],
    );
  }

  /// Sign-in status, or a key field — never both.
  Widget _authRow(ColorScheme scheme) {
    if (_provider == ProviderKind.scripted) {
      return Text(
        'The demo bot follows a fixed script, so you can try a room before '
        'setting anything up.',
        style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
      );
    }
    if (_provider.needsApiKey) {
      final saved =
          ModelCatalog.credentials?.forProvider(_provider)?.kind ==
          CredentialKind.apiKey;
      if (saved && _key.text.isEmpty && !_replaceKey) {
        return Row(
          children: [
            const Icon(Icons.check_circle_outline, size: 18),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                'API key saved securely',
                style: const TextStyle(fontSize: 13),
              ),
            ),
            TextButton(
              onPressed: () => setState(() => _replaceKey = true),
              child: const Text('Change'),
            ),
          ],
        );
      }
      return TextField(
        controller: _key,
        obscureText: true,
        autocorrect: false,
        enableSuggestions: false,
        decoration: InputDecoration(
          labelText: 'API key',
          helperText: 'Saved securely for all your ${_provider.label} bots',
        ),
        // A key unlocks the live model list, so reload once it is entered.
        onSubmitted: (_) => _loadModels(refresh: true),
        onEditingComplete: () => _loadModels(refresh: true),
      );
    }
    if (widget.desktopClis && _provider.cli != null) {
      final cli = _provider.cli!;
      final status = findCli(cli);
      return StatusPill(
        color: status.available ? scheme.tertiary : Colors.orange,
        label: status.available
            ? '${cli.binary} installed'
            : '${cli.binary} not found',
        icon: status.available ? Icons.terminal : Icons.error_outline,
      );
    }
    if (widget.desktopClis && !_isSupportedDesktopProvider) {
      return const Text('Choose a supported provider above.');
    }

    // A signed-in subscription works everywhere, including a phone with
    // no CLIs, because the engine calls the provider's API directly.
    final paired = ModelCatalog.credentials?.forProvider(_provider);
    if (paired != null) {
      final verified = !_loadingModels && _models?.source == ModelSource.live;
      return StatusPill(
        color: verified ? scheme.tertiary : Colors.orange,
        label: _models?.source == ModelSource.needsSignIn
            ? 'Sign in again from Accounts'
            : !verified
            ? (_loadingModels
                  ? 'Checking saved sign-in…'
                  : 'Sign-in saved · model access unavailable')
            : 'Signed in' +
                  (paired.plan != null ? ' — ${paired.plan}' : '') +
                  ' · no API key needed',
        icon: Icons.verified_user_outlined,
      );
    }

    final cli = _provider.cli!;
    if (widget.desktopClis) {
      return StatusPill(
        color: Colors.orange,
        label: '${cli.binary} not found',
        icon: Icons.error_outline,
      );
    }
    // Not signed in, and no CLI to sign in with. Where the provider offers a
    // device-code flow that is the fix; otherwise the CLI is the only route.
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        StatusPill(
          color: Colors.orange,
          label: _provider.signsInOnDevice ? 'Not signed in' : 'CLI not found',
          icon: Icons.error_outline,
        ),
        const SizedBox(height: 6),
        Text(
          _provider.signsInOnDevice
              ? 'Open Sign in and approve ${_provider.label} with the account '
                    'you already pay for. No API key, and nothing to install.'
              : '${cli.binary} runs on Linux only. Install it and sign in, or bring a '
                    'sign-in from a machine that has it — run `orgbot pair` '
                    'there and import it from Sign in.',
          style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
        ),
      ],
    );
  }

  Widget _modelField(ColorScheme scheme) {
    if (_loadingModels) {
      return Row(
        children: [
          const SizedBox(
            width: 14,
            height: 14,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
          const SizedBox(width: 10),
          Text(
            'Loading models…',
            style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
          ),
        ],
      );
    }
    final list = _models;
    if (list == null) return const SizedBox.shrink();

    if (list.options.isEmpty && !_custom) {
      // Nothing can be listed yet — say why rather than inventing options.
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            widget.desktopClis && list.source == ModelSource.needsKey
                ? 'Add an API key in Setup to load models.'
                : list.problem ?? 'No models available yet.',
            style: TextStyle(fontSize: 12, color: Colors.orange),
          ),
          const SizedBox(height: 6),
          TextButton.icon(
            onPressed: () => setState(() => _custom = true),
            icon: const Icon(Icons.edit, size: 15),
            label: const Text('Enter a model id instead'),
          ),
        ],
      );
    }
    final value = _custom
        ? _customValue
        : (list.options.any((o) => o.id == _model)
              ? _model
              : (list.options.isEmpty ? _customValue : list.options.first.id));

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropdownButtonFormField<String>(
          initialValue: value,
          key: ValueKey('${_provider.key}:$value'),
          isExpanded: true,
          itemHeight: null,
          selectedItemBuilder: (_) => [
            for (final option in list.options)
              Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  option.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            const Align(
              alignment: Alignment.centerLeft,
              child: Text('Custom…'),
            ),
          ],
          decoration: InputDecoration(
            labelText: 'Model',
            suffixIcon: IconButton(
              tooltip: 'Reload from the provider',
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: () {
                ModelCatalog.invalidate();
                _loadModels(refresh: true);
              },
            ),
          ),
          items: [
            for (final option in list.options)
              DropdownMenuItem(
                value: option.id,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(option.label, overflow: TextOverflow.ellipsis),
                    if (option.note.isNotEmpty)
                      Text(
                        option.note,
                        style: TextStyle(
                          fontSize: 11,
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
            const DropdownMenuItem(value: _customValue, child: Text('Custom…')),
          ],
          onChanged: (v) => setState(() {
            if (v == _customValue) {
              _custom = true;
            } else {
              _custom = false;
              _model = v ?? _model;
            }
          }),
        ),
        if (_custom) ...[
          const SizedBox(height: 10),
          TextField(
            controller: _customModel,
            decoration: const InputDecoration(
              labelText: 'Model id',
              hintText: 'exact id from your provider',
            ),
          ),
        ],
        const SizedBox(height: 6),
        Text(
          widget.desktopClis && _provider.cli != null
              ? (findCli(_provider.cli!).available
                    ? 'Models via ${_provider.cli!.binary} CLI.'
                    : '${_provider.cli!.binary} CLI not found.')
              : list.source == ModelSource.live
              ? 'Loaded from ${_provider.label}${_provider.needsApiKey ? ' API' : ''}.'
              : list.problem ?? 'Could not load the model list. Try again.',
          style: TextStyle(
            fontSize: 11,
            color: list.source == ModelSource.live
                ? scheme.onSurfaceVariant
                : Colors.orange,
          ),
        ),
      ],
    );
  }

  Future<void> _save() async {
    if (_saving) return;
    final credentials = ModelCatalog.credentials;
    final keepingUnavailable =
        widget.desktopClis &&
        !_isSupportedDesktopProvider &&
        _provider == widget.existing?.provider;
    if (widget.desktopClis && _provider.needsApiKey && credentials == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Secure API key storage is unavailable.')),
      );
      return;
    }
    if (!keepingUnavailable &&
        _provider.needsApiKey &&
        _key.text.trim().isEmpty &&
        ModelCatalog.credentials?.isSignedIn(_provider) != true) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            'Add your ${_provider.label} key in ${widget.desktopClis ? 'Setup' : 'Accounts'} or enter it here.',
          ),
        ),
      );
      return;
    }
    if (_name.text.trim().isEmpty) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Give the bot a name')));
      return;
    }
    if (!keepingUnavailable &&
        (_custom ||
            _provider == ProviderKind.ollama ||
            (!widget.desktopClis && _provider != ProviderKind.scripted)) &&
        _resolvedModel.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Enter a model id, or pick one above')),
      );
      return;
    }
    setState(() => _saving = true);
    try {
      if (_provider.needsApiKey &&
          credentials != null &&
          _key.text.trim().isNotEmpty) {
        await credentials.saveApiKey(_provider, _key.text);
      }
      await widget.onSave(
        BotDef(
          id: widget.existing?.id,
          name: _name.text.trim(),
          handle: _handle.text.trim(),
          persona: _persona.text.trim(),
          provider: _provider,
          model: _resolvedModel,
          apiKey:
              _provider.needsApiKey &&
                  credentials == null &&
                  !widget.desktopClis
              ? _key.text.trim()
              : widget.desktopClis && _provider == widget.existing?.provider
              ? widget.existing?.apiKey ?? ''
              : '',
          role: _role,
          additionalRoles: _additionalRoles
              .where((role) => role != _role)
              .toList(),
          powers: _powers,
        ),
      );
      if (mounted) Navigator.pop(context);
    } catch (_) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text(
              'Could not save this bot. Your existing settings were kept.',
            ),
          ),
        );
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }
}

// Both editors need the whole phone width; desktop keeps a compact dialog.
Widget _editor(
  BuildContext context, {
  required Widget title,
  required Widget content,
  required List<Widget> actions,
}) {
  if (MediaQuery.sizeOf(context).width >= 600) {
    return AlertDialog(title: title, content: content, actions: actions);
  }
  return Dialog.fullscreen(
    child: Scaffold(
      appBar: AppBar(
        toolbarHeight: 68,
        title: title,
        leading: const CloseButton(),
        actions: [
          Padding(
            padding: const EdgeInsets.only(right: 16),
            child: actions.last,
          ),
        ],
      ),
      body: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 12, 20, 12),
          child: content,
        ),
      ),
    ),
  );
}
