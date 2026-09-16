on run argv
    set mountPath to item 1 of argv
    set outputPath to item 2 of argv
    set instructionText to item 3 of argv
    tell application "Finder"
        set targetWindow to front window
        set windowTarget to POSIX path of (target of targetWindow as alias)
        if windowTarget is not mountPath and windowTarget is not (mountPath & "/") then error "front Finder window is not the mounted DMG"
        set appItem to item "TelevyBackup.app" of targetWindow
        set applicationsItem to item "Applications" of targetWindow
        set appPosition to position of appItem
        set applicationsPosition to position of applicationsItem
        set direction to "right"
        if (item 1 of applicationsPosition) is less than or equal to (item 1 of appPosition) then set direction to "left"
        set q to ASCII character 34
        set payload to "{" & q & "app_name" & q & ":" & q & "TelevyBackup.app" & q & "," & q & "applications_name" & q & ":" & q & "Applications" & q & "," & q & "app_position" & q & ":[" & (item 1 of appPosition) & "," & (item 2 of appPosition) & "]," & q & "applications_position" & q & ":[" & (item 1 of applicationsPosition) & "," & (item 2 of applicationsPosition) & "]," & q & "drag_direction" & q & ":" & q & direction & q & "," & q & "instruction" & q & ":" & q & instructionText & q & "," & q & "window_role" & q & ":" & q & "Finder" & q & "}"
    end tell
    do shell script "/usr/bin/printf %s " & quoted form of payload & " > " & quoted form of outputPath
end run
