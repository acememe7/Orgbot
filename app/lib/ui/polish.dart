import 'package:flutter/material.dart';
import 'package:flutter/cupertino.dart' show CupertinoPageTransitionsBuilder;

Duration motionDuration(BuildContext context, [int milliseconds = 280]) =>
    MediaQuery.disableAnimationsOf(context)
    ? Duration.zero
    : Duration(milliseconds: milliseconds);

/// A tactile surface. Scaling never participates in list measurement.
class PressSurface extends StatefulWidget {
  const PressSurface({
    super.key,
    required this.child,
    required this.onTap,
    this.padding = const EdgeInsets.all(20),
  });
  final Widget child;
  final VoidCallback onTap;
  final EdgeInsetsGeometry padding;
  @override
  State<PressSurface> createState() => _PressSurfaceState();
}

class _PressSurfaceState extends State<PressSurface> {
  bool _pressed = false;
  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return AnimatedScale(
      scale: _pressed ? 0.982 : 1,
      duration: motionDuration(context, _pressed ? 100 : 220),
      curve: Curves.easeOutCubic,
      child: Material(
        color: scheme.surfaceContainerLow,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(24),
          side: BorderSide(color: scheme.outlineVariant.withValues(alpha: 0.6)),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: widget.onTap,
          onTapDown: (_) => setState(() => _pressed = true),
          onTapUp: (_) => setState(() => _pressed = false),
          onTapCancel: () => setState(() => _pressed = false),
          splashFactory: NoSplash.splashFactory,
          child: Padding(padding: widget.padding, child: widget.child),
        ),
      ),
    );
  }
}

class FormSection extends StatelessWidget {
  const FormSection({super.key, required this.title, required this.children});
  final String title;
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 24),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 0, 4, 10),
          child: Text(
            title,
            style: Theme.of(context).textTheme.labelLarge?.copyWith(
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        Material(
          color: Theme.of(context).colorScheme.surfaceContainerLow,
          borderRadius: BorderRadius.circular(20),
          clipBehavior: Clip.antiAlias,
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: children,
            ),
          ),
        ),
      ],
    ),
  );
}

/// Keeps navigation state and in-progress account sign-in alive between pages.
class RetainedPage extends StatefulWidget {
  const RetainedPage({super.key, required this.child});
  final Widget child;
  @override
  State<RetainedPage> createState() => _RetainedPageState();
}

class _RetainedPageState extends State<RetainedPage>
    with AutomaticKeepAliveClientMixin {
  @override
  bool get wantKeepAlive => true;
  @override
  Widget build(BuildContext context) {
    super.build(context);
    return widget.child;
  }
}

class WorkspaceNavigation extends StatelessWidget {
  const WorkspaceNavigation({
    super.key,
    required this.selected,
    required this.onSelected,
    this.desktop = false,
  });
  final int selected;
  final ValueChanged<int> onSelected;
  final bool desktop;
  static const labels = ['Rooms', 'Bots', 'Accounts'];
  static const desktopLabels = ['Rooms', 'Bots', 'Setup'];
  static const icons = [
    Icons.chat_bubble_outline_rounded,
    Icons.people_outline_rounded,
    Icons.tune_rounded,
  ];
  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return SafeArea(
      top: false,
      child: Container(
        margin: const EdgeInsets.fromLTRB(20, 8, 20, 12),
        padding: const EdgeInsets.all(5),
        decoration: BoxDecoration(
          color: scheme.surfaceContainerLow,
          borderRadius: BorderRadius.circular(28),
          border: Border.all(
            color: scheme.outlineVariant.withValues(alpha: 0.6),
          ),
        ),
        child: SizedBox(
          height: 60,
          child: Stack(
            children: [
              AnimatedAlign(
                alignment: Alignment(selected - 1.0, 0),
                duration: motionDuration(context, 320),
                curve: Curves.easeOutCubic,
                child: FractionallySizedBox(
                  widthFactor: 1 / 3,
                  heightFactor: 1,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: scheme.surfaceContainerHighest,
                      borderRadius: BorderRadius.circular(23),
                    ),
                  ),
                ),
              ),
              Row(
                children: [
                  for (var i = 0; i < desktopLabels.length; i++)
                    Expanded(
                      child: Semantics(
                        selected: selected == i,
                        button: true,
                        label: desktop ? desktopLabels[i] : labels[i],
                        onTap: () => onSelected(i),
                        child: ExcludeSemantics(
                          child: InkWell(
                            borderRadius: BorderRadius.circular(23),
                            onTap: () => onSelected(i),
                            child: Column(
                              mainAxisAlignment: MainAxisAlignment.center,
                              children: [
                                AnimatedScale(
                                  scale: selected == i ? 1 : 0.92,
                                  duration: motionDuration(context, 220),
                                  child: Icon(
                                    icons[i],
                                    size: 22,
                                    color: selected == i
                                        ? scheme.primary
                                        : scheme.onSurfaceVariant,
                                  ),
                                ),
                                const SizedBox(height: 4),
                                Text(
                                  desktop ? desktopLabels[i] : labels[i],
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    fontSize: 11,
                                    fontWeight: selected == i
                                        ? FontWeight.w600
                                        : FontWeight.w400,
                                    color: selected == i
                                        ? scheme.onSurface
                                        : scheme.onSurfaceVariant,
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The indicator follows the same animation as the swipeable room pages.
class RoomTabs extends StatelessWidget {
  const RoomTabs({super.key});
  static const labels = ['Chat', 'Work', 'Files', 'Memory'];
  @override
  Widget build(BuildContext context) {
    final controller = DefaultTabController.of(context);
    final scheme = Theme.of(context).colorScheme;
    if (MediaQuery.textScalerOf(context).scale(13) > 19) {
      return TabBar(
        isScrollable: true,
        tabAlignment: TabAlignment.start,
        tabs: [for (final label in labels) Tab(text: label)],
      );
    }
    return Container(
      height: 46,
      margin: const EdgeInsets.fromLTRB(20, 4, 20, 12),
      padding: const EdgeInsets.all(4),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(16),
      ),
      child: AnimatedBuilder(
        animation: controller.animation!,
        builder: (context, _) => Stack(
          children: [
            Align(
              alignment: Alignment(-1 + 2 * controller.animation!.value / 3, 0),
              child: FractionallySizedBox(
                widthFactor: 0.25,
                heightFactor: 1,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: scheme.surfaceContainerHighest,
                    borderRadius: BorderRadius.circular(12),
                  ),
                ),
              ),
            ),
            Row(
              children: [
                for (var i = 0; i < labels.length; i++)
                  Expanded(
                    child: Semantics(
                      selected: controller.index == i,
                      button: true,
                      child: InkWell(
                        borderRadius: BorderRadius.circular(12),
                        onTap: () => controller.animateTo(
                          i,
                          duration: motionDuration(context, 320),
                          curve: Curves.easeOutCubic,
                        ),
                        child: Center(
                          child: Text(
                            labels[i],
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: controller.index == i
                                  ? FontWeight.w600
                                  : FontWeight.w400,
                              color: controller.index == i
                                  ? scheme.onSurface
                                  : scheme.onSurfaceVariant,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

Future<T?> showEditor<T>({
  required BuildContext context,
  required WidgetBuilder builder,
}) => showGeneralDialog<T>(
  context: context,
  barrierDismissible: true,
  barrierLabel: 'Close editor',
  barrierColor: Colors.black54,
  transitionDuration: motionDuration(context, 320),
  pageBuilder: (context, _, _) => builder(context),
  transitionBuilder: (context, animation, _, child) {
    final curved = animation.drive(CurveTween(curve: Curves.easeOutCubic));
    return FadeTransition(
      opacity: curved,
      child: SlideTransition(
        position: curved.drive(
          Tween(begin: const Offset(0, 0.045), end: Offset.zero),
        ),
        child: child,
      ),
    );
  },
);

/// Native route motion, with a stationary page when reduced motion is enabled.
class WorkspacePageTransitions extends CupertinoPageTransitionsBuilder {
  const WorkspacePageTransitions();
  @override
  Widget buildTransitions<T>(
    PageRoute<T> route,
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) => MediaQuery.disableAnimationsOf(context)
      ? child
      : super.buildTransitions(
          route,
          context,
          animation,
          secondaryAnimation,
          child,
        );
}
