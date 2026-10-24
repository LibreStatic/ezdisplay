import QtQuick
import org.kde.kwin

// Animation benchmark, loaded by bench.sh. Every 800 ms one event on the output whose name starts
// with @OUTPUT@: 8 x quick-tile preview (outline shown, then hidden), 8 x maximize/restore of the
// EZD-DRAG editor window, then 3 s of moves. Each event is logged as "EZDANIM <kind> <n>" so it
// can be matched with the host trace.
Item {
    id: root
    property var win: null
    property rect out
    property bool found: false
    property int step: 0
    readonly property string target: "@OUTPUT@"
    readonly property int ww: Math.min(1100, out.width * 0.46)
    readonly property int wh: Math.min(800, out.height * 0.6)
    Timer {
        interval: 800
        repeat: true
        running: true
        onTriggered: {
            if (!root.found) {
                const ss = Workspace.screens;
                for (let i = 0; i < ss.length; i++)
                    if (ss[i].name.startsWith(root.target)) {
                        root.out = ss[i].geometry;
                        root.found = true;
                    }
                if (!root.found) {
                    console.log("EZDANIM no-output " + root.target);
                    return;
                }
            }
            if (!root.win) {
                const ws = Workspace.stackingOrder;
                for (let i = 0; i < ws.length; i++)
                    if (ws[i].caption.indexOf("EZD-DRAG") >= 0) root.win = ws[i];
                if (!root.win) return;
                root.win.frameGeometry = Qt.rect(root.out.x + root.out.width * 0.2,
                    root.out.y + root.out.height * 0.2, root.ww, root.wh);
                return;
            }
            const s = root.step++;
            if (s < 16) {
                if (s % 2 == 0) {
                    Workspace.showOutline(root.out);
                    console.log("EZDANIM outline-show " + s);
                } else {
                    Workspace.hideOutline();
                    console.log("EZDANIM outline-hide " + s);
                }
            } else if (s < 32) {
                const on = s % 2 == 0;
                root.win.setMaximize(on, on);
                console.log("EZDANIM " + (on ? "maximize " : "restore ") + s);
            } else if (s == 32) {
                console.log("EZDANIM move-start " + s);
                mover.running = true;
            } else if (s == 36) {
                mover.running = false;
                console.log("EZDANIM end " + s);
                running = false;
            }
        }
    }
    // Main-thread watchdog: logs when KWin's event loop ran this timer late.
    Timer {
        interval: 10
        repeat: true
        running: true
        property double last: 0
        onTriggered: {
            const now = Date.now();
            if (last > 0 && now - last > 30) console.log("EZDTICK " + (now - last));
            last = now;
        }
    }
    Timer {
        id: mover
        interval: 16
        repeat: true
        property int k: 0
        onTriggered: {
            const f = (k++ % 120) / 120;
            root.win.frameGeometry = Qt.rect(root.out.x + 100 + f * (root.out.width - root.ww - 200),
                root.out.y + root.out.height * 0.16, root.ww, root.wh);
        }
    }
}
