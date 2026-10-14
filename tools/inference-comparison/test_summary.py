import unittest
from summarize import decision, distribution, summarize

class SummaryTests(unittest.TestCase):
    def test_comparison_report_completeness(self):
        self.assertEqual(decision({})['status'], 'incomplete')
        groups = {name+'/cuda/combined': {'completed':5, 'requested':5,
                  'measurement_contract_valid':False} for name in
                  ['runner-baseline','runner-ort-v3','runner-burn']}
        self.assertEqual(decision(groups)['status'], 'incomplete')

    def test_comparison_gate_failures(self):
        groups = {}
        for name in ['runner-baseline','runner-ort-v3','runner-burn']:
            groups[name+'/cuda/combined']={'completed':5,'requested':5,
                'measurement_contract_valid':True, 'rtf':{'median':.5},
                'scores':[{'wer':.1,'der':.2}]*5}
        self.assertTrue(all(decision(groups)['gates'].values()))
        groups['runner-burn/cuda/combined']['rtf']['median']=1.1
        self.assertEqual(decision(groups)['recommendation'],'retain ORT')
        groups['runner-burn/cuda/combined']['scores']=[{'wer':.2,'der':.3}]*5
        self.assertFalse(decision(groups)['gates']['C_B_WER'])
        self.assertFalse(decision(groups)['gates']['C_B_DER'])
        groups['runner-baseline/cuda/combined']['requested']=6
        self.assertEqual(decision(groups)['status'],'incomplete')

    def test_distribution(self):
        self.assertIsNone(distribution([]))
        self.assertEqual(distribution([1,2,3,4,5])['median'],3)

if __name__=='__main__':
    unittest.main()
