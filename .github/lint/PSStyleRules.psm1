using namespace System.Management.Automation.Language

Set-StrictMode -Version Latest

function New-StyleRecord {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord])]
    param(
        [Parameter(Mandatory)][string] $Rule,
        [Parameter(Mandatory)][IScriptExtent] $Extent,
        [string] $Detail = ''
    )
    [Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord]@{
        Message  = $script:Messages[$Rule] -f $Detail
        Extent   = $Extent
        RuleName = $Rule
        Severity = 'Warning'
    }
}

function Get-LineExtent {
    [CmdletBinding()]
    [OutputType([ScriptExtent])]
    param(
        [Parameter(Mandatory)][string] $File,
        [Parameter(Mandatory)][AllowEmptyString()][string[]] $Lines,
        [Parameter(Mandatory)][int] $Number
    )
    $text = $Lines[$Number - 1]
    $start = [ScriptPosition]::new($File, $Number, 1, $text)
    $end = [ScriptPosition]::new($File, $Number, $text.Length + 1, $text)
    [ScriptExtent]::new($start, $end)
}

function Get-ParameterName {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $Command,
        [Parameter(Mandatory)][string] $Given
    )
    if (-not $script:ParameterCache.ContainsKey($Command)) {
        $info = Get-Command -Name $Command -ErrorAction SilentlyContinue
        $script:ParameterCache[$Command] = @()
        if ($info -and $info.Parameters) {
            $script:ParameterCache[$Command] = @($info.Parameters.Values)
        }
    }
    $hits = [System.Collections.Generic.List[string]]::new()
    foreach ($p in $script:ParameterCache[$Command]) {
        $names = @($p.Name) + @($p.Aliases)
        if ($names -contains $Given) {
            return $p.Name
        }
        foreach ($n in $names) {
            if ($n.StartsWith($Given, [StringComparison]::OrdinalIgnoreCase) -and -not $hits.Contains($p.Name)) {
                $hits.Add($p.Name)
            }
        }
    }
    if ($hits.Count -eq 1) {
        return $hits[0]
    }
    ''
}

function Test-HasParameter {
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][CommandAst] $Command,
        [Parameter(Mandatory)][string[]] $Name
    )
    $commandName = $Command.GetCommandName()
    foreach ($element in $Command.CommandElements) {
        if ($element -is [VariableExpressionAst] -and $element.Splatted) {
            return $true
        }
        if ($element -is [CommandParameterAst]) {
            if ((Get-ParameterName -Command $commandName -Given $element.ParameterName) -in $Name) {
                return $true
            }
        }
    }
    $false
}

function Test-HeaderStatement {
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][Ast] $Statement
    )
    $Statement.Extent.Text -in $script:HeaderLines
}

function Get-ScopeAst {
    [CmdletBinding()]
    [OutputType([Ast])]
    param(
        [Parameter(Mandatory)][Ast] $Node
    )
    $p = $Node.Parent
    while ($p) {
        if ($p -is [ScriptBlockAst] -and ($null -eq $p.Parent -or $p.Parent -is [FunctionDefinitionAst])) {
            return $p
        }
        $p = $p.Parent
    }
    $Node
}

function Test-InLoop {
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][Ast] $Node,
        [Parameter(Mandatory)][Ast] $Scope
    )
    $p = $Node.Parent
    while ($p -and $p -ne $Scope) {
        if ($p -is [LoopStatementAst]) {
            return $true
        }
        $p = $p.Parent
    }
    $false
}

function Get-AssignedKind {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][AssignmentStatementAst] $Assignment
    )
    $left = $Assignment.Left
    if ($left -is [ConvertExpressionAst]) {
        $type = $left.Type.TypeName
        if ($type.IsArray -or $type.Name -eq 'array') {
            return 'array'
        }
        if ($type.Name -eq 'string') {
            return 'string'
        }
        return 'other'
    }
    $right = $Assignment.Right
    if ($right -is [CommandExpressionAst]) {
        $value = $right.Expression
        if ($value -is [ArrayExpressionAst] -or $value -is [ArrayLiteralAst]) {
            return 'array'
        }
        if ($value -is [StringConstantExpressionAst] -or $value -is [ExpandableStringExpressionAst]) {
            return 'string'
        }
    }
    'other'
}

function Get-AssignedName {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][ExpressionAst] $Left
    )
    $target = $Left
    if ($target -is [ConvertExpressionAst]) {
        $target = $target.Child
    }
    if ($target -is [VariableExpressionAst]) {
        return $target.VariablePath.UserPath
    }
    ''
}

function Get-FileRecord {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord])]
    param(
        [Parameter(Mandatory)][ScriptBlockAst] $Root,
        [Parameter(Mandatory)][string] $File,
        [Parameter(Mandatory)][AllowEmptyCollection()][Ast[]] $Statements
    )
    $raw = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($File))
    $lines = $raw -split "`n"
    $match = [regex]::Match($raw, '[^\x00-\x7F]')
    if ($match.Success) {
        $number = [regex]::Matches($raw.Substring(0, $match.Index), "`n").Count + 1
        New-StyleRecord -Rule 'StyleAsciiOnly' -Extent (Get-LineExtent -File $File -Lines $lines -Number $number)
    }
    $tokens = $null
    $parseErrors = $null
    $null = [Parser]::ParseFile($File, [ref]$tokens, [ref]$parseErrors)
    foreach ($t in $tokens) {
        if ($t.Kind -eq [TokenKind]::LineContinuation) {
            New-StyleRecord -Rule 'StyleLineContinuation' -Extent $t.Extent
        } elseif ($t.Kind -eq [TokenKind]::Comment) {
            $shebang = $t.Extent.StartOffset -eq 0 -and $t.Text.StartsWith('#!')
            if (-not $shebang -and $t.Text -notmatch '^#requires\s') {
                New-StyleRecord -Rule 'StyleNoComment' -Extent $t.Extent
            }
        }
    }
    if (-not $File.EndsWith('.ps1', [StringComparison]::OrdinalIgnoreCase)) {
        return
    }
    $param = $Root.ParamBlock
    $binding = $param -and @($param.Attributes | Where-Object { $_.TypeName.Name -eq 'CmdletBinding' }).Count -gt 0
    $splat = {
        param([Ast] $Node)
        $Node -is [VariableExpressionAst] -and $Node.Splatted -and $Node.VariablePath.UserPath -eq 'args'
    }
    $launcher = -not $param -and $null -ne $Root.Find($splat, $false)
    $header = $lines[0].TrimEnd("`r") -eq '#!/usr/bin/env pwsh' -and ($binding -or $launcher) -and
    $Statements.Count -ge 2 -and
    $Statements[0].Extent.Text -eq $script:HeaderLines[0] -and $Statements[1].Extent.Text -eq $script:HeaderLines[1]
    if (-not $header) {
        New-StyleRecord -Rule 'StyleScriptHeader' -Extent (Get-LineExtent -File $File -Lines $lines -Number 1)
    }
    $last = $null
    if ($Statements.Count) {
        $last = $Statements[-1]
    }
    if (-not ($last -is [ExitStatementAst] -and $last.Pipeline)) {
        New-StyleRecord -Rule 'StyleScriptExit' -Extent $Root.Extent
    }
}

function Get-CommandRecord {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord])]
    param(
        [Parameter(Mandatory)][CommandAst] $Command
    )
    if ($Command.InvocationOperator -eq [TokenKind]::Dot) {
        New-StyleRecord -Rule 'StyleDotSource' -Extent $Command.Extent
    }
    $name = $Command.GetCommandName()
    if (-not $name) {
        return
    }
    $pipe = $Command.Parent
    $fromValue = $pipe -is [PipelineAst] -and $pipe.PipelineElements.Count -gt 1 -and
    $pipe.PipelineElements[0] -is [CommandExpressionAst] -and $pipe.PipelineElements[1] -eq $Command
    switch ($name) {
        'ConvertTo-Json' {
            if (-not (Test-HasParameter -Command $Command -Name 'Depth')) {
                New-StyleRecord -Rule 'StyleJsonDepth' -Extent $Command.Extent
            }
        }
        { $_ -in 'Out-File', 'Set-Content', 'Add-Content' } {
            New-StyleRecord -Rule 'StyleFileWrite' -Extent $Command.Extent -Detail $name
        }
        'Get-Content' {
            if (-not (Test-HasParameter -Command $Command -Name 'Wait', 'Tail')) {
                New-StyleRecord -Rule 'StyleFileRead' -Extent $Command.Extent
            }
        }
        'Out-Null' {
            New-StyleRecord -Rule 'StyleDiscard' -Extent $Command.Extent -Detail 'Out-Null'
        }
        'Write-Error' {
            New-StyleRecord -Rule 'StyleWriteError' -Extent $Command.Extent
        }
        'Import-Module' {
            $rooted = '^\$PSScriptRoot'
            foreach ($element in $Command.CommandElements) {
                $text = ''
                if ($element -is [StringConstantExpressionAst] -and $element -ne $Command.CommandElements[0]) {
                    $text = $element.Value
                } elseif ($element -is [ExpandableStringExpressionAst] -and $element.Value -notmatch $rooted) {
                    $text = $element.Value
                }
                if ($text -match '[\\/]|\.(psm1|psd1|dll)$') {
                    New-StyleRecord -Rule 'StyleModulePath' -Extent $element.Extent -Detail $text
                }
            }
        }
        { $_ -in 'ForEach-Object', '%', 'foreach' } {
            if ($fromValue) {
                New-StyleRecord -Rule 'StyleForEachObject' -Extent $Command.Extent
            }
        }
    }
}

function Get-FunctionRecord {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord])]
    param(
        [Parameter(Mandatory)][FunctionDefinitionAst] $Function
    )
    $block = $Function.Body.ParamBlock
    $names = @()
    if ($block) {
        $names = @(foreach ($attribute in $block.Attributes) { $attribute.TypeName.Name })
    }
    if (-not $block -or $names -notcontains 'CmdletBinding' -or $names -notcontains 'OutputType') {
        $whole = $Function.Extent
        $line = $whole.StartScriptPosition.Line
        $start = [ScriptPosition]::new($whole.File, $whole.StartLineNumber, $whole.StartColumnNumber, $line)
        $extent = [ScriptExtent]::new($start, $start)
        New-StyleRecord -Rule 'StyleFunctionHeader' -Extent $extent -Detail $Function.Name
    }
}

function Measure-StyleRule {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord[]])]
    param(
        [Parameter(Mandatory)][ScriptBlockAst] $ScriptBlockAst
    )
    if ($null -ne $ScriptBlockAst.Parent) {
        return
    }
    $file = $ScriptBlockAst.Extent.File
    $statements = @()
    if ($ScriptBlockAst.EndBlock) {
        $statements = @($ScriptBlockAst.EndBlock.Statements)
    }
    if ($file -and (Test-Path -LiteralPath $file)) {
        Get-FileRecord -Root $ScriptBlockAst -File $file -Statements $statements
    }

    $body = $false
    foreach ($s in $statements) {
        if ($s -is [FunctionDefinitionAst]) {
            if ($body) {
                New-StyleRecord -Rule 'StyleLayout' -Extent $s.Extent
            }
        } elseif (-not (Test-HeaderStatement -Statement $s)) {
            $body = $true
        }
    }

    $all = $ScriptBlockAst.FindAll({ $true }, $true)
    $kinds = [System.Collections.Generic.Dictionary[string, System.Collections.Generic.HashSet[string]]]::new()
    foreach ($a in $all) {
        if ($a -is [AssignmentStatementAst] -and $a.Operator -eq [TokenKind]::Equals) {
            $name = Get-AssignedName -Left $a.Left
            if ($name) {
                $key = '{0}:{1}' -f (Get-ScopeAst -Node $a).Extent.StartOffset, $name.ToLowerInvariant()
                if (-not $kinds.ContainsKey($key)) {
                    $kinds[$key] = [System.Collections.Generic.HashSet[string]]::new()
                }
                $null = $kinds[$key].Add((Get-AssignedKind -Assignment $a))
            }
        }
    }

    foreach ($n in $all) {
        if ($n -is [CommandAst]) {
            Get-CommandRecord -Command $n
        } elseif ($n -is [FunctionDefinitionAst]) {
            Get-FunctionRecord -Function $n
        } elseif ($n -is [FileRedirectionAst]) {
            $target = $n.Location
            if (-not ($target -is [VariableExpressionAst] -and $target.VariablePath.UserPath -eq 'null')) {
                New-StyleRecord -Rule 'StyleFileWrite' -Extent $n.Extent -Detail '> or >>'
            }
        } elseif ($n -is [VariableExpressionAst]) {
            if ($n.VariablePath.UserPath -in 'IsWindows', 'IsLinux', 'IsMacOS', 'IsCoreCLR') {
                New-StyleRecord -Rule 'StylePlatformCheck' -Extent $n.Extent -Detail $n.VariablePath.UserPath
            }
        } elseif ($n -is [ParameterAst]) {
            if (-not @($n.Attributes | Where-Object { $_ -is [TypeConstraintAst] }).Count) {
                New-StyleRecord -Rule 'StyleTypedParameter' -Extent $n.Extent
            }
        } elseif ($n -is [ExitStatementAst]) {
            $p = $n.Parent
            while ($p -and -not ($p -is [FunctionDefinitionAst])) {
                $p = $p.Parent
            }
            if ($p) {
                New-StyleRecord -Rule 'StyleExitInFunction' -Extent $n.Extent
            }
        } elseif ($n -is [CatchClauseAst]) {
            if ($n.IsCatchAll) {
                New-StyleRecord -Rule 'StyleTypedCatch' -Extent $n.Extent
            }
        } elseif ($n -is [AttributeAst]) {
            if ($n.TypeName.FullName -match '(^|\.)SuppressMessage(Attribute)?$') {
                New-StyleRecord -Rule 'StyleSuppression' -Extent $n.Extent
            }
        } elseif ($n -is [ConvertExpressionAst]) {
            if ($n.Type.TypeName.Name -eq 'void') {
                New-StyleRecord -Rule 'StyleDiscard' -Extent $n.Extent -Detail '[void]'
            }
        } elseif ($n -is [AssignmentStatementAst] -and $n.Operator -eq [TokenKind]::PlusEquals) {
            $name = Get-AssignedName -Left $n.Left
            if ($name) {
                $scope = Get-ScopeAst -Node $n
                $key = '{0}:{1}' -f $scope.Extent.StartOffset, $name.ToLowerInvariant()
                $set = $null
                if ($kinds.TryGetValue($key, [ref]$set)) {
                    if ($set.Contains('array')) {
                        New-StyleRecord -Rule 'StyleArrayAppend' -Extent $n.Extent
                    } elseif ($set.Contains('string') -and (Test-InLoop -Node $n -Scope $scope)) {
                        New-StyleRecord -Rule 'StyleStringAppend' -Extent $n.Extent
                    }
                }
            }
        }
    }
}

$script:ParameterCache = @{}
$script:HeaderLines = @('$ErrorActionPreference = ''Stop''', 'Set-StrictMode -Version Latest')
$script:Messages = @{
    StyleAsciiOnly        = 'PowerShell files are ASCII, with no BOM (PowerShell section 0).'
    StyleLineContinuation = 'Splat instead of ending a line with a backtick (PS-9).'
    StyleNoComment        = 'Remove the comment; names and types carry the meaning (GEN-29).'
    StyleScriptHeader     = 'Start the script from merge/template.ps1 (PS-1).'
    StyleScriptExit       = 'End the script with exit <code> (PS-13).'
    StyleLayout           = 'Define every function before the main body (PS-21).'
    StyleDotSource        = 'Import shared code from a .psm1 with Import-Module; never dot-source (PS-22).'
    StyleModulePath       = 'Import a module file by full path built from $PSScriptRoot, not ''{0}'' (PS-22).'
    StyleJsonDepth        = 'Pass -Depth to ConvertTo-Json (PS-10).'
    StyleFileWrite        = 'Write files with [IO.File]::WriteAllText or AppendAllText, not {0} (PS-4).'
    StyleFileRead         = 'Read files with [IO.File]::ReadAllText; Get-Content only with -Wait or -Tail (PS-4).'
    StyleDiscard          = 'Discard output with $null =, not {0} (PS-7).'
    StyleWriteError       = 'Throw, or write to [Console]::Error and exit 1; never Write-Error (PS-16).'
    StyleFunctionHeader   = 'Start {0} with [CmdletBinding()], [OutputType()] and param() (PS-12).'
    StyleForEachObject    = 'Loop over a value already in memory with foreach, not ForEach-Object (section 4).'
    StylePlatformCheck    = '${0} does not exist on 5.1; check $env:OS -eq ''Windows_NT'' (PS-11).'
    StyleTypedParameter   = 'Give every parameter a type (PS-12).'
    StyleExitInFunction   = 'Functions throw; only the script calls exit (PS-13).'
    StyleTypedCatch       = 'Catch a specific exception type (PS-14).'
    StyleArrayAppend      = 'Collect into a List or assign the loop output; never += on an array (section 4).'
    StyleSuppression      = 'Fix the code; never suppress a rule inside a file (PowerShell section 1).'
    StyleStringAppend     = 'Build strings in a loop with a StringBuilder, not += (section 4).'
}

Export-ModuleMember -Function Measure-StyleRule
