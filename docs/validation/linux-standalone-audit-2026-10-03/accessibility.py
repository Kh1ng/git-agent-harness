import pyatspi
count = 0
def walk(node, depth=0):
    global count
    if depth > 14 or count > 700: return
    count += 1
    try:
        name = node.name
        if name: print("GUI:",node.getRoleName(),repr(name))
        try:
            text = node.queryText().getText(0,-1)
            if text and text != name: print("TEXT:",repr(text))
        except Exception: pass
        for child in node: walk(child,depth+1)
    except Exception: pass
walk(pyatspi.Registry.getDesktop(0))
