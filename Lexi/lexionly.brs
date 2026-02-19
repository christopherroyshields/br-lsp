00020      dim Infile$*255, OutFile$*255
00100      library "lexi" : fnApplyLexi
00105      print "Applying Lexi preprocessor in BR "&wbversion$
00110      let fnApplyLexi(InFile$,OutFile$,1)
