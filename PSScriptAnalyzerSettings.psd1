@{
    # ParseError too: without it a script that does not parse reports nothing.
    Severity     = @('ParseError', 'Error', 'Warning')
    ExcludeRules = @(
        'PSAvoidUsingWriteHost' # the output of these scripts is for a person in a console, not for a pipeline
        'PSReviewUnusedParameter' # it does not see a script parameter read inside a function of the script
        'PSUseShouldProcessForStateChangingFunctions' # private helpers of a script: -WhatIf and -Confirm are not part of any script's interface
    )
}
